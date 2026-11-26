//! A whole sync on one SSH connection: one login, so one card signature.
//!
//! A send fetches, integrates and pushes. If the remote moves between the
//! fetch and the push, the push is refused (paw never forces); it then
//! fetches, integrates and pushes once more, still on the same connection.
//! A refresh only fetches and integrates.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use super::integrate::{IntegrateReport, Integration, Replaced, integrate_locked};
use super::transport::{ConnectError, Connection, Remote};
use super::{PushError, SyncError, TransferCallback, current_branch, fetch_branches, git_err, push_tip};
use crate::key::KeyError;
use crate::fault::Fault;
use crate::remote::RemoteFailure;
use crate::lock::{Exclusive, StoreLock};
use crate::key::SshSigner;
use crate::ssh::SshPublicKey;

/// Where a sync ended.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SyncState {
    /// The store and the remote hold the same history.
    UpToDate,
    /// Fetched only; this many local changes are still waiting.
    Waiting { unpushed: u32 },
    /// The remote's history was replaced. Nothing here was changed; ask the
    /// person, then call `accept_replaced_remote`.
    RemoteReplaced { replaced: Replaced },
}

impl SyncState {
    fn waiting(unpushed: u32) -> Self {
        if unpushed == 0 { Self::UpToDate } else { Self::Waiting { unpushed } }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncReport {
    pub state: SyncState,
    /// What fetching brought in, over every round.
    pub integration: IntegrateReport,
    /// Local changes the remote now has.
    pub pushed: u32,
    /// The first push was refused because the remote moved, and it was
    /// fetched, integrated and pushed again.
    pub retried: bool,
    /// Card signatures the whole sync cost: one per login.
    pub signatures: u32,
}

impl SyncReport {
    fn new() -> Self {
        Self {
            state: SyncState::UpToDate,
            integration: IntegrateReport::default(),
            pushed: 0,
            retried: false,
            signatures: 0,
        }
    }

    /// Integrates what was fetched into the store at `dir`, adding to this
    /// report. Returns how many changes are waiting then, or `None` if the
    /// remote's history was replaced (the state says so).
    fn integrate(&mut self, held: &Exclusive, dir: &Path) -> Result<Option<u32>, SyncError> {
        match integrate_locked(held, dir)? {
            Integration::Integrated(this_round) => {
                self.integration.absorb(this_round);
                let waiting = waiting(dir)?;
                self.state = SyncState::waiting(waiting);
                Ok(Some(waiting))
            }
            Integration::RemoteReplaced(replaced) => {
                self.state = SyncState::RemoteReplaced { replaced };
                Ok(None)
            }
        }
    }
}

/// Counts the signatures a login asks for.
struct Counting<'a> {
    inner: &'a dyn SshSigner,
    count: AtomicU32,
}

impl SshSigner for Counting<'_> {
    fn public_key(&self) -> &SshPublicKey {
        self.inner.public_key()
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, KeyError> {
        let signature = self.inner.sign(data)?;
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(signature)
    }
}

/// Called with the store's lock held.
fn waiting(dir: &Path) -> Result<u32, SyncError> {
    let repo = super::guard::open(dir)?;
    Ok(super::queue::waiting(&repo)?.commits.len() as u32)
}

fn fetch_and_integrate(
    connection: &Connection,
    store: &StoreLock,
    dir: &Path,
    report: &mut SyncReport,
    progress: Option<TransferCallback<'_>>,
) -> Result<(Option<u32>, Exclusive), SyncError> {
    let repo = super::guard::open(dir)?;
    let (_, held) = fetch_branches(connection, store, &repo, progress)?;
    let waiting = report.integrate(&held, dir)?;
    Ok((waiting, held))
}

fn branch_tip(dir: &Path) -> Result<(git2::Repository, String, git2::Oid), SyncError> {
    let repo = super::guard::open(dir)?;
    let (branch, _) = current_branch(&repo)?;
    let tip = repo.refname_to_id(&branch).map_err(git_err)?;
    Ok((repo, branch, tip))
}

/// Whether a waiting change must not be pushed now, asked after each
/// integrate.
pub(crate) type HoldsBack<'a> = dyn Fn() -> Result<bool, SyncError> + 'a;

/// `progress` for one step, keeping it for the next.
fn reborrow<'b>(progress: &'b mut Option<TransferCallback<'_>>) -> Option<TransferCallback<'b>> {
    match progress {
        Some(progress) => Some(&mut **progress),
        None => None,
    }
}

/// Fetches, integrates and, if `send`, pushes on one login, for the store
/// at `dir`. Retries once, on the same connection, if the remote moved
/// before the push. After integrating, if `holds_back` says a waiting
/// change is not encrypted for exactly its entry's accepted recipients
/// (`crate::write::held_back`), nothing is pushed and the state is
/// [`SyncState::Waiting`]. Stops between steps once `stop` says so, with
/// the key's `Cancelled`, and reports each fetch's progress to `progress`.
///
/// Fails with the key's own error if the key did not sign the login, else
/// with the sync's.
pub(crate) fn run_until<E: From<KeyError> + From<SyncError>>(
    remote: &Remote,
    signer: &dyn SshSigner,
    dir: &Path,
    holds_back: &HoldsBack<'_>,
    send: bool,
    stop: &dyn Fn() -> bool,
    progress: Option<TransferCallback<'_>>,
) -> Result<SyncReport, E> {
    let counting = Counting { inner: signer, count: AtomicU32::new(0) };
    let connection = match Connection::open(remote, &counting) {
        Ok(connection) => connection,
        Err(ConnectError::Key(error)) => return Err(error.into()),
        Err(ConnectError::Fault(error)) => return Err(SyncError::from(error).into()),
    };
    let Some(mut report) = run_on(&connection, dir, holds_back, send, stop, progress)? else {
        return Err(KeyError::Cancelled.into());
    };
    report.signatures = counting.count.load(Ordering::SeqCst);
    Ok(report)
}

/// [`run_until`] on a connection already logged in. A stop ends it with
/// `None`, only between whole steps (after the login, after a fetch and
/// its integrate, before a push): nothing is left half done.
fn run_on(
    connection: &Connection,
    dir: &Path,
    holds_back: &HoldsBack<'_>,
    send: bool,
    stop: &dyn Fn() -> bool,
    mut progress: Option<TransferCallback<'_>>,
) -> Result<Option<SyncReport>, SyncError> {
    if stop() {
        return Ok(None);
    }
    let mut report = SyncReport::new();

    // What is and is not under the store lock: the login and every
    // transfer run without it; publishing the fetched refs, integrating,
    // counting and checking the waiting changes, and recording a push
    // run with it.
    let store = StoreLock::for_repo(dir);
    let pushing = if send { Some(store.pushing(crate::lock::FETCH_WAIT)?) } else { None };
    let (waiting, held) = fetch_and_integrate(connection, &store, dir, &mut report, reborrow(&mut progress))?;
    let Some(mut waiting) = waiting else {
        return Ok(Some(report));
    };
    if send && waiting > 0 && stop() {
        return Ok(None);
    }
    // A waiting change encrypted for recipients its entry no longer has
    // is not sent: nothing is, until the person has dealt with it.
    if !send || waiting == 0 || holds_back()? {
        return Ok(Some(report));
    }
    let Some(pushing) = &pushing else {
        return Err(Fault::local("internal: a send without its push lock").into());
    };
    let (repo, branch, tip) = branch_tip(dir)?;

    match push_tip(connection, &store, &repo, &branch, tip, held, pushing, crate::lock::RECORD_WAIT) {
        Ok(()) => {}
        Err(PushError::NotFastForward) => {
            if stop() {
                return Ok(None);
            }
            report.retried = true;
            let (again, held) =
                fetch_and_integrate(connection, &store, dir, &mut report, reborrow(&mut progress))?;
            let Some(again) = again else {
                return Ok(Some(report));
            };
            waiting = again;
            if waiting > 0 && holds_back()? {
                return Ok(Some(report));
            }
            let (repo, branch, tip) = branch_tip(dir)?;
            if waiting > 0 {
                match push_tip(connection, &store, &repo, &branch, tip, held, pushing, crate::lock::RECORD_WAIT) {
                    Ok(()) => {}
                    Err(PushError::NotFastForward) => return Err(RemoteFailure::KeptMoving.into()),
                    Err(PushError::Failed(error)) => return Err(error.into()),
                }
            }
        }
        Err(PushError::Failed(error)) => return Err(error.into()),
    }
    report.pushed = waiting;
    report.state = SyncState::UpToDate;
    Ok(Some(report))
}

/// After a fetch by some other transport (GitHub's HTTPS, in
/// `crate::github`): integrates and reports exactly as a refresh does
/// after its own fetch, so both paths give the same results for the same
/// remote (recipients changed, replaced history, refused trees).
pub(crate) fn integrate_fetched(held: &Exclusive, dir: &Path) -> Result<SyncReport, SyncError> {
    let mut report = SyncReport::new();
    report.integrate(held, dir)?;
    Ok(report)
}

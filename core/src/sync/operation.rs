//! A whole sync on one SSH connection: one login, so one card signature.
//!
//! A send fetches, integrates and pushes. If the remote moves between the
//! fetch and the push, the push is refused (paw never forces); it then
//! fetches, integrates and pushes once more, still on the same connection.
//! A refresh only fetches and integrates.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use super::integrate::{IntegrateReport, Integration, Replaced, integrate_locked};
use super::transport::{Connection, Remote};
use super::{SyncError, TransferCallback, current_branch, fetch_branches, git_err, push_tip};
use crate::PawError;
use crate::lock::{Exclusive, StoreLock};
use crate::ssh::{SshPublicKey, SshSigner};

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

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, PawError> {
        let signature = self.inner.sign(data)?;
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(signature)
    }
}

/// A connection error as sync words it: a host key mismatch becomes
/// [`SyncError::HostKeyChanged`] with both fingerprints.
pub(crate) fn connect_error(remote: &Remote, error: PawError) -> SyncError {
    match error {
        PawError::HostKeyMismatch { presented } => SyncError::HostKeyChanged {
            host: remote.host.clone(),
            expected: remote.host_key.fingerprints(),
            presented,
        },
        other => other.into(),
    }
}

/// How many changes are waiting ([`super::queue::waiting`]). Called with
/// the store's lock held.
fn waiting(dir: &Path) -> Result<u32, SyncError> {
    let repo = super::guard::open(dir)?;
    Ok(super::queue::waiting(&repo)?.commits.len() as u32)
}

/// Fetches on `connection` (no store lock during the transfer), then with
/// the lock publishes and integrates into `report`. Returns what
/// [`SyncReport::integrate`] does, and the lock, still held.
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

/// The current branch and its tip.
fn branch_tip(dir: &Path) -> Result<(git2::Repository, String, git2::Oid), SyncError> {
    let repo = super::guard::open(dir)?;
    let (branch, _) = current_branch(&repo)?;
    let tip = repo.refname_to_id(&branch).map_err(git_err)?;
    Ok((repo, branch, tip))
}

/// What a sync that was asked to stop returns. Nothing is left half done:
/// it stops only between whole steps (after the login, after a fetch and
/// its integrate, before a push).
fn stopped() -> SyncError {
    SyncError::Failed { message: "stopped".into() }
}

/// Whether any waiting change is held back (see [`super::queue::held_back`]).
fn holding_back(dir: &Path, state_dir: &Path) -> Result<bool, SyncError> {
    Ok(!super::queue::held_back(dir, state_dir)?.is_empty())
}

/// `progress` for one step, keeping it for the next.
fn reborrow<'b>(progress: &'b mut Option<TransferCallback<'_>>) -> Option<TransferCallback<'b>> {
    match progress {
        Some(progress) => Some(&mut **progress),
        None => None,
    }
}

/// Fetches, integrates and, if `send`, pushes on one login, for the store
/// at `dir` whose state directory is `state_dir`. Retries once, on the
/// same connection, if the remote moved before the push. After
/// integrating, if any waiting change is not encrypted for exactly its
/// entry's accepted recipients ([`super::queue::held_back`]), nothing is
/// pushed and the state is [`SyncState::Waiting`]. Stops between steps
/// once `stop` says so, and reports each fetch's progress to `progress`.
pub(crate) fn run_until(
    remote: &Remote,
    signer: &dyn SshSigner,
    dir: &Path,
    state_dir: &Path,
    send: bool,
    stop: &dyn Fn() -> bool,
    mut progress: Option<TransferCallback<'_>>,
) -> Result<SyncReport, SyncError> {
    let counting = Counting { inner: signer, count: AtomicU32::new(0) };
    let connection = Connection::open(remote, &counting).map_err(|e| connect_error(remote, e))?;
    if stop() {
        return Err(stopped());
    }
    let mut report = SyncReport::new();
    let finish = |mut report: SyncReport| {
        report.signatures = counting.count.load(Ordering::SeqCst);
        report
    };

    // What is and is not under the store lock: the login and every
    // transfer run without it; publishing the fetched refs, integrating,
    // counting and checking the waiting changes, and recording a push
    // run with it.
    let store = StoreLock::for_repo(dir);
    // One send at a time per store, from before it reads what to push
    // until the result is recorded; a refresh does not take it.
    let pushing = if send { Some(store.pushing(crate::lock::FETCH_WAIT)?) } else { None };
    let (waiting, held) = fetch_and_integrate(&connection, &store, dir, &mut report, reborrow(&mut progress))?;
    let Some(mut waiting) = waiting else {
        return Ok(finish(report));
    };
    if send && waiting > 0 && stop() {
        return Err(stopped());
    }
    // A waiting change encrypted for recipients its entry no longer has
    // is not sent: nothing is, until the person has dealt with it.
    if !send || waiting == 0 || holding_back(dir, state_dir)? {
        return Ok(finish(report));
    }
    let Some(pushing) = &pushing else {
        return Err(SyncError::Failed { message: "internal: a send without its push lock".into() });
    };
    let (repo, branch, tip) = branch_tip(dir)?;

    match push_tip(&connection, &store, &repo, &branch, tip, held, pushing) {
        Ok(()) => {}
        Err(PawError::NotFastForward) => {
            if stop() {
                return Err(stopped());
            }
            report.retried = true;
            let (again, held) =
                fetch_and_integrate(&connection, &store, dir, &mut report, reborrow(&mut progress))?;
            let Some(again) = again else {
                return Ok(finish(report));
            };
            waiting = again;
            if waiting > 0 && holding_back(dir, state_dir)? {
                return Ok(finish(report));
            }
            let (repo, branch, tip) = branch_tip(dir)?;
            if waiting > 0 {
                match push_tip(&connection, &store, &repo, &branch, tip, held, pushing) {
                    Err(PawError::NotFastForward) => return Err(SyncError::RemoteKeptMoving),
                    other => other?,
                }
            }
        }
        Err(other) => return Err(other.into()),
    }
    report.pushed = waiting;
    report.state = SyncState::UpToDate;
    Ok(finish(report))
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

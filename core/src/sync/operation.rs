//! A whole sync on one SSH connection: one login, so one card signature.
//!
//! A send fetches, integrates and pushes. If the remote moves between the
//! fetch and the push, the push is refused (paw never forces); it then
//! fetches, integrates and pushes once more, still on the same connection.
//! A refresh only fetches and integrates.

use super::integrate::{self, IntegrateReport, Integration, Plan, Replaced};
use super::transport::{ConnectError, Connection, Remote};
use super::{OpenStore, PushError, SyncError, TransferCallback, current_branch, fetch_branches, git_err, push_tip};
use crate::files::layout::StoreDirs;
use crate::key::KeyError;
use crate::remote::RemoteFailure;
use crate::lock::{Exclusive, Shared};
use crate::key::{Halt, SshSigner};

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
}

impl SyncReport {
    fn new() -> Self {
        Self {
            state: SyncState::UpToDate,
            integration: IntegrateReport::default(),
            pushed: 0,
            retried: false,
        }
    }

    /// Integrates what was fetched into `store` as `plan` says, adding to
    /// this report. Returns how many changes are waiting then, or `None` if
    /// the remote's history was replaced (the state says so).
    fn integrate(&mut self, held: &Exclusive, store: &OpenStore, plan: Plan) -> Result<Option<u32>, SyncError> {
        match integrate::apply(held, store, plan)? {
            Integration::Integrated(this_round) => {
                self.integration.absorb(this_round);
                let waiting = waiting(held, store)?;
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

fn waiting(reading: &Shared, store: &OpenStore) -> Result<u32, SyncError> {
    Ok(super::queue::waiting(reading, store)?.commits.len() as u32)
}

fn fetch_and_integrate(
    connection: &Connection,
    store: &OpenStore,
    report: &mut SyncReport,
    progress: Option<TransferCallback<'_>>,
) -> Result<(Option<u32>, Exclusive), SyncError> {
    let (_, held) = fetch_branches(connection, store, progress)?;
    let waiting = report.integrate(&held, store, integrate::plan(&held, store)?)?;
    Ok((waiting, held))
}

fn branch_tip(store: &OpenStore) -> Result<(String, git2::Oid), SyncError> {
    let (branch, _) = current_branch(&store.repo)?;
    let tip = store.repo.refname_to_id(&branch).map_err(git_err)?;
    Ok((branch, tip))
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
/// `dirs`. Retries once, on the same connection, if the remote moved
/// before the push. After integrating, if `holds_back` says a waiting
/// change is not encrypted for exactly its entry's accepted recipients
/// (`crate::write::held_back`), nothing is pushed and the state is
/// [`SyncState::Waiting`]. Stops between steps once `stop` says so, with
/// [`Halt::Cancelled`], and reports each fetch's progress to `progress`.
///
/// Fails with the key's own error if the key did not sign the login, else
/// with the sync's.
pub(crate) fn run_until<E: From<KeyError> + From<Halt> + From<SyncError>>(
    remote: &Remote,
    signer: &dyn SshSigner,
    dirs: &StoreDirs,
    holds_back: &HoldsBack<'_>,
    send: bool,
    stop: &dyn Fn() -> bool,
    progress: Option<TransferCallback<'_>>,
) -> Result<SyncReport, E> {
    let connection = match Connection::open(remote, signer) {
        Ok(connection) => connection,
        Err(ConnectError::Key(error)) => return Err(error.into()),
        Err(ConnectError::Cancelled) => return Err(Halt::Cancelled.into()),
        Err(ConnectError::Fault(error)) => return Err(SyncError::from(error).into()),
    };
    let store = OpenStore::open(dirs).map_err(SyncError::from)?;
    run_on(&connection, &store, holds_back, send, stop, progress)?.ok_or_else(|| Halt::Cancelled.into())
}

/// How often a send pushes: once, and once more after fetching again if
/// the remote moved before the first push.
const SEND_ROUNDS: u32 = 2;

/// [`run_until`] on a connection already logged in. A stop ends it with
/// `None`, only between whole steps (after the login, after a fetch and
/// its integrate, before a push): nothing is left half done.
fn run_on(
    connection: &Connection,
    store: &OpenStore,
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
    let pushing = if send { Some(store.dirs.lock().pushing(crate::lock::FETCH_WAIT)?) } else { None };
    let (mut waiting, mut held) = fetch_and_integrate(connection, store, &mut report, reborrow(&mut progress))?;
    let Some(pushing) = pushing else {
        return Ok(Some(report));
    };
    let mut round = 1;
    loop {
        let Some(count) = waiting.filter(|&count| count > 0) else {
            return Ok(Some(report));
        };
        if stop() {
            return Ok(None);
        }
        if holds_back()? {
            return Ok(Some(report));
        }
        let (branch, tip) = branch_tip(store)?;
        match push_tip(connection, store, &branch, tip, held, &pushing, crate::lock::RECORD_WAIT) {
            Ok(()) => {
                report.pushed = count;
                report.state = SyncState::UpToDate;
                return Ok(Some(report));
            }
            Err(PushError::NotFastForward) if round == SEND_ROUNDS => return Err(RemoteFailure::KeptMoving.into()),
            Err(PushError::NotFastForward) => {}
            Err(PushError::Failed(error)) => return Err(error.into()),
        }
        if stop() {
            return Ok(None);
        }
        report.retried = true;
        (waiting, held) = fetch_and_integrate(connection, store, &mut report, reborrow(&mut progress))?;
        round += 1;
    }
}

/// After a fetch by some other transport (GitHub's HTTPS, in
/// `crate::github`): integrates and reports exactly as a refresh does
/// after its own fetch, so both paths give the same results for the same
/// remote (recipients changed, replaced history, refused trees).
pub(crate) fn integrate_fetched(held: &Exclusive, store: &OpenStore, plan: Plan) -> Result<SyncReport, SyncError> {
    let mut report = SyncReport::new();
    report.integrate(held, store, plan)?;
    Ok(report)
}

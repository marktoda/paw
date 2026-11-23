//! A whole sync on one SSH connection: one login, so one card signature.
//!
//! [`send`] fetches, integrates and pushes. If the remote moves between the
//! fetch and the push, the push is refused (paw never forces); it then
//! fetches, integrates and pushes once more, still on the same connection.
//! [`refresh`] only fetches and integrates.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use super::history::commits_between;
use super::integrate::{IntegrateReport, Integration, Replaced, integrate};
use super::transport::{Connection, Remote};
use super::{SyncError, current_branch, fetch, git_err, push};
use crate::PawError;
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

/// Opens one connection, turning a host key mismatch into
/// [`SyncError::HostKeyChanged`] with both fingerprints.
pub fn connect(remote: &Remote, signer: &dyn SshSigner) -> Result<Connection, SyncError> {
    Connection::open(remote, signer).map_err(|e| connect_error(remote, e))
}

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

/// Local commits the remote-tracking branch does not have.
fn unpushed(dir: &Path) -> Result<u32, SyncError> {
    let repo = super::guard::open(dir)?;
    let (branch, short) = current_branch(&repo)?;
    let head = repo.refname_to_id(&branch).map_err(git_err)?;
    let Ok(remote) = repo.refname_to_id(&format!("refs/remotes/origin/{short}")) else {
        return Ok(0);
    };
    Ok(commits_between(&repo, head, &[remote])?.len() as u32)
}

/// Fetches and integrates on `connection`. `Err(replaced)` when the remote's
/// history was replaced.
fn fetch_and_integrate(
    connection: &Connection,
    dir: &Path,
    report: &mut IntegrateReport,
) -> Result<Result<(), Replaced>, SyncError> {
    fetch(connection, dir)?;
    match integrate(dir)? {
        Integration::Integrated(this_round) => {
            report.absorb(this_round);
            Ok(Ok(()))
        }
        Integration::RemoteReplaced(replaced) => Ok(Err(replaced)),
    }
}

/// What a sync that was asked to stop returns. Nothing is left half done:
/// it stops only between whole steps (after the login, after a fetch and
/// its integrate, before a push).
fn stopped() -> SyncError {
    SyncError::Failed { message: "stopped".into() }
}

fn run(
    remote: &Remote,
    signer: &dyn SshSigner,
    dir: &Path,
    send: bool,
    stop: &dyn Fn() -> bool,
) -> Result<SyncReport, SyncError> {
    let counting = Counting { inner: signer, count: AtomicU32::new(0) };
    let connection = connect(remote, &counting)?;
    if stop() {
        return Err(stopped());
    }
    let mut report = SyncReport {
        state: SyncState::UpToDate,
        integration: IntegrateReport::default(),
        pushed: 0,
        retried: false,
        signatures: 0,
    };
    let finish = |mut report: SyncReport, state| {
        report.state = state;
        report.signatures = counting.count.load(Ordering::SeqCst);
        report
    };

    if let Err(replaced) = fetch_and_integrate(&connection, dir, &mut report.integration)? {
        return Ok(finish(report, SyncState::RemoteReplaced { replaced }));
    }
    let mut waiting = unpushed(dir)?;
    if send && waiting > 0 && stop() {
        return Err(stopped());
    }
    if !send || waiting == 0 {
        let state = if waiting == 0 { SyncState::UpToDate } else { SyncState::Waiting { unpushed: waiting } };
        return Ok(finish(report, state));
    }

    match push(&connection, dir) {
        Ok(()) => {}
        Err(PawError::NotFastForward) => {
            if stop() {
                return Err(stopped());
            }
            report.retried = true;
            if let Err(replaced) = fetch_and_integrate(&connection, dir, &mut report.integration)? {
                return Ok(finish(report, SyncState::RemoteReplaced { replaced }));
            }
            waiting = unpushed(dir)?;
            if waiting > 0 {
                match push(&connection, dir) {
                    Err(PawError::NotFastForward) => return Err(SyncError::RemoteKeptMoving),
                    other => other?,
                }
            }
        }
        Err(other) => return Err(other.into()),
    }
    report.pushed = waiting;
    Ok(finish(report, SyncState::UpToDate))
}

/// Fetches, integrates and pushes on one login. Retries once, on the same
/// connection, if the remote moved before the push.
pub fn send(remote: &Remote, signer: &dyn SshSigner, dir: &Path) -> Result<SyncReport, SyncError> {
    run(remote, signer, dir, true, &|| false)
}

/// Fetches and integrates on one login; pushes nothing.
pub fn refresh(remote: &Remote, signer: &dyn SshSigner, dir: &Path) -> Result<SyncReport, SyncError> {
    run(remote, signer, dir, false, &|| false)
}

/// [`send`] (or [`refresh`] if `push` is false) that stops between steps
/// once `stop` says so.
pub(crate) fn run_until(
    remote: &Remote,
    signer: &dyn SshSigner,
    dir: &Path,
    push: bool,
    stop: &dyn Fn() -> bool,
) -> Result<SyncReport, SyncError> {
    run(remote, signer, dir, push, stop)
}

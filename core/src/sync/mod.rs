//! Keeping the local copy of the password store in step with the git host.
//!
//! A push only ever fast-forwards. If the remote has moved, it fails with
//! [`PushError::NotFastForward`] and changes nothing.

pub mod address;
pub(crate) mod guard;
pub(crate) mod history;
pub mod hosts;
pub mod integrate;
pub mod operation;
pub mod queue;
mod transport;

use std::time::{Duration, Instant};

use git2::{AutotagOption, Direction, FetchOptions, PushOptions, RemoteCallbacks, Repository};

pub use history::{ChangeKind, KeptBoth};
pub use integrate::{IntegrateReport, Integration, RecipientsChange, Replaced, accept_replaced_remote};
pub use transport::{ConnectError, Connection, HostKeyPin, PresentedHostKey, Remote, key_type_words, scan_host_key};

use crate::PawError;
use crate::fault::Fault;
use crate::files::layout::StoreDirs;
use crate::files::refs;
use crate::lock::{DEFAULT_WAIT, Exclusive, FETCH_WAIT, Fetching};
use crate::remote::{RefusedPath, RemoteFailure};

/// Why a sync, an integrate or a change to the queue did not happen. In
/// every case nothing in the store was changed.
#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum SyncError {
    /// The local branch has commits paw did not make (another tool used
    /// this copy). paw does not guess what to do with them.
    #[error("this copy has a change paw did not make ({commit}: {reason})")]
    NotPawHistory { commit: String, reason: String },
    /// Files in the working copy differ from the last commit. paw never
    /// leaves it that way, so something else wrote there; nothing is
    /// overwritten.
    #[error("the store has changes that were never saved: {}", .paths.join(", "))]
    UncommittedChanges { paths: Vec<String> },
    /// The change to discard is being sent right now. Nothing was changed.
    #[error("this change is being sent right now; wait for the send to finish")]
    BeingSent,
    /// No waiting change has this id (it was sent, or discarded).
    #[error("{id} is not an unpushed change")]
    NotWaiting { id: String },
    /// Re-encrypting found the waiting changes changed under it, again and
    /// again. Those already done stay done.
    #[error("the waiting changes kept changing while paw was re-encrypting them; try again")]
    QueueKeptChanging,
    /// Underneath the sync: the store, the remote, a refusal or a stop.
    /// The store and the waiting changes stay as they were.
    #[error(transparent)]
    Fault { fault: Fault },
}

crate::fault::workflow_error!(SyncError);

/// How far a clone or fetch has got, for a progress bar. Return `false`
/// to stop.
pub type TransferCallback<'a> = &'a mut dyn FnMut(TransferProgress) -> bool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    pub received_objects: u32,
    /// 0 until the host has said.
    pub total_objects: u32,
    pub received_bytes: u64,
}

pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Decides which of libgit2's progress updates (one per object, and more
/// while it indexes) are reported: the first, then at most one per
/// [`PROGRESS_INTERVAL`], plus the moment the last object arrives, so a
/// bar does not stop short of the end. A repeat of the last report is never
/// sent, and once [`Self::stop`] is called (the listener said stop, or the
/// work was cancelled) nothing is.
#[derive(Debug, Clone, Default)]
pub struct ProgressThrottle {
    last: Option<(Instant, TransferProgress)>,
    stopped: bool,
}

impl ProgressThrottle {
    pub fn should_report(&mut self, now: Instant, update: TransferProgress) -> bool {
        if self.stopped {
            return false;
        }
        let report = match self.last {
            None => true,
            Some((_, last)) if last == update => false,
            Some((_, last)) if update.total_objects > 0 && update.received_objects == update.total_objects => {
                last.received_objects != update.received_objects
            }
            Some((at, _)) => now.saturating_duration_since(at) >= PROGRESS_INTERVAL,
        };
        if report {
            self.last = Some((now, update));
        }
        report
    }

    pub fn stop(&mut self) {
        self.stopped = true;
    }

    pub fn stopped(&self) -> bool {
        self.stopped
    }
}

/// libgit2's progress callback for `report`, throttled: `report` returning
/// `false` stops the transfer, and it is not called again.
fn throttled<'a>(report: TransferCallback<'a>) -> impl FnMut(git2::Progress<'_>) -> bool + 'a {
    let mut throttle = ProgressThrottle::default();
    move |stats| {
        if throttle.stopped() {
            return false;
        }
        let update = TransferProgress {
            received_objects: u32::try_from(stats.received_objects()).unwrap_or(u32::MAX),
            total_objects: u32::try_from(stats.total_objects()).unwrap_or(u32::MAX),
            received_bytes: stats.received_bytes() as u64,
        };
        if !throttle.should_report(Instant::now(), update) {
            return true;
        }
        let go_on = report(update);
        if !go_on {
            throttle.stop();
        }
        go_on
    }
}

/// What a clone found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cloned {
    /// The default branch is checked out.
    Store,
    /// The repository has no commits. `dir` holds an empty repository.
    Empty,
    /// The default branch holds something paw will not check out (see
    /// [`refused_remote`]). Nothing was checked out; `dir` holds the
    /// fetched objects only.
    Refused(Vec<RefusedPath>),
}

pub(crate) fn git_err(e: git2::Error) -> PawError {
    PawError::Git(e.message().to_owned())
}

/// A store's repository, opened for one operation with paw's settings
/// applied ([`guard::open`]).
pub(crate) struct OpenStore {
    pub(crate) dirs: StoreDirs,
    pub(crate) repo: Repository,
}

impl OpenStore {
    pub(crate) fn open(dirs: &StoreDirs) -> Result<Self, PawError> {
        Ok(Self { dirs: dirs.clone(), repo: guard::open(dirs.repo_dir())? })
    }
}

/// Every fetch of a store, whatever its transport. Under the store's fetch
/// lock (waiting up to `fetch_wait`: one fetch at a time writes objects and
/// the private refs, so two never write one pack index), what an earlier
/// fetch left in the private namespace is cleared and `transfer` fills it,
/// without the store lock: saves and reads go on meanwhile. Then, with the
/// store lock (waiting up to `store_wait()`), what it received becomes
/// `origin/*`. Returns what `transfer` returned and the store lock, held.
pub(crate) fn fetch_into<T, E: From<PawError>>(
    store: &OpenStore,
    fetch_wait: Duration,
    store_wait: impl FnOnce() -> Duration,
    transfer: impl FnOnce(&Repository) -> Result<T, E>,
) -> Result<(T, Exclusive), E> {
    let fetching = store.dirs.lock().fetching(fetch_wait)?;
    crate::lock::remove_stale_fetch_locks(store.dirs.repo_dir());
    let repo = &store.repo;
    for name in fetched_refs(repo)?.into_iter().map(|(name, _)| name) {
        if let Ok(mut reference) = repo.find_reference(&name) {
            let _ = reference.delete();
        }
    }
    let transferred = transfer(repo)?;
    let held = store.dirs.lock().exclusive_within(store_wait())?;
    publish_fetched(&held, store, fetching)?;
    Ok((transferred, held))
}

fn fetched_refs(repo: &Repository) -> Result<Vec<(String, git2::Oid)>, PawError> {
    let mut found = Vec::new();
    for reference in repo.references().map_err(git_err)? {
        let reference = reference.map_err(git_err)?;
        if let (Some(name), Some(target)) = (reference.name().ok(), reference.target())
            && refs::fetched_branch(name).is_some()
        {
            found.push((name.to_owned(), target));
        }
    }
    Ok(found)
}

/// The ref updates are the critical section, so the store lock is held;
/// the fetch lock is released at the end.
fn publish_fetched(held: &Exclusive, store: &OpenStore, fetching: Fetching) -> Result<(), PawError> {
    debug_assert!(held.covers(store.dirs.lock()));
    let repo = &store.repo;
    let fetched = fetched_refs(repo)?;
    for (name, target) in &fetched {
        let branch = refs::fetched_branch(name).unwrap_or_default();
        repo.reference(&refs::remote_tracking(branch), *target, true, "fetch")
            .map_err(git_err)?;
    }
    for (name, _) in &fetched {
        if let Ok(mut reference) = repo.find_reference(name) {
            let _ = reference.delete();
        }
    }
    drop(fetching);
    Ok(())
}

/// [`fetch_into`] over SSH: returns the remote's default branch, if it has
/// one, and the store lock, held.
pub(crate) fn fetch_branches(
    connection: &Connection,
    store: &OpenStore,
    progress: Option<TransferCallback<'_>>,
) -> Result<(Option<String>, Exclusive), Fault> {
    fetch_into(store, FETCH_WAIT, || DEFAULT_WAIT, |repo| connection.with_remote(repo, |remote| fetch_connected(remote, progress)))
}

/// Fetches all branches through `remote`, whose transport the caller has
/// chosen (SSH here, GitHub's HTTPS in `crate::github`), and returns the
/// remote's default branch, if it has one. Both transports fetch exactly
/// this way.
pub(crate) fn fetch_connected(
    remote: &mut git2::Remote<'_>,
    progress: Option<TransferCallback<'_>>,
) -> Result<Option<String>, git2::Error> {
    let mut connected = remote.connect_auth(Direction::Fetch, None, None)?;
    // An empty repository has no default branch; that is not an error.
    let default = connected
        .default_branch()
        .ok()
        .and_then(|name| name.as_str().map(str::to_owned).ok());
    let mut options = FetchOptions::new();
    options.download_tags(AutotagOption::None);
    // FETCH_HEAD is not used: nothing is written into it (libgit2 still
    // truncates it at each fetch, which takes no lock file).
    options.update_fetchhead(false);
    if let Some(progress) = progress {
        let mut callbacks = RemoteCallbacks::new();
        callbacks.transfer_progress(throttled(progress));
        options.remote_callbacks(callbacks);
    }
    connected
        .remote()
        .fetch(&[refs::fetch_refspec()], Some(&mut options), None)?;
    Ok(default)
}

/// The branch `HEAD` points at, as `(refs/heads/<name>, <name>)`.
pub(crate) fn current_branch(repo: &Repository) -> Result<(String, String), PawError> {
    let head = repo.find_reference("HEAD").map_err(git_err)?;
    let not_on_branch = || PawError::Git("HEAD is not on a branch".into());
    let full = head.symbolic_target().map_err(git_err)?.ok_or_else(not_on_branch)?;
    let short = refs::short(full).ok_or_else(not_on_branch)?;
    Ok((full.to_owned(), short.to_owned()))
}

/// Clones the remote's default branch into the store's repository, which
/// must be empty or not exist yet. An empty repository is not an error
/// here.
pub fn clone_with_transfer(
    connection: &Connection,
    remote: &Remote,
    dirs: &StoreDirs,
    progress: Option<TransferCallback<'_>>,
) -> Result<Cloned, Fault> {
    let repo = {
        let _held = dirs.exclusive()?;
        let repo = guard::init(dirs.repo_dir())?;
        // Recorded so the copy is readable with ordinary git tools. paw itself
        // always goes through a `Connection`.
        repo.remote("origin", &remote.url()).map_err(git_err)?;
        repo
    };
    let store = OpenStore { dirs: dirs.clone(), repo };
    let repo = &store.repo;

    let (branch, held) = fetch_branches(connection, &store, progress)?;
    let Some(branch) = branch else {
        return Ok(Cloned::Empty);
    };
    let short = refs::short(&branch).ok_or_else(|| PawError::Git(format!("unexpected default branch {branch}")))?;
    let tip = repo.refname_to_id(&refs::remote_tracking(short)).map_err(git_err)?;
    let tree = repo.find_commit(tip).map_err(git_err)?.tree_id();
    let refused = guard::check_tree(repo, tree).map_err(git_err)?;
    if !refused.is_empty() {
        return Ok(Cloned::Refused(refused));
    }

    repo.reference(&branch, tip, false, "clone").map_err(git_err)?;
    repo.reference(&refs::last_seen(short), tip, true, "clone")
        .map_err(git_err)?;
    repo.set_head(&branch).map_err(git_err)?;
    repo.checkout_head(Some(&mut guard::checkout())).map_err(git_err)?;
    drop(held);
    Ok(Cloned::Store)
}

/// What the fetched tip of the current branch's remote counterpart holds
/// that paw will not check out; empty if nothing (or nothing was fetched).
/// A sync that failed with [`RemoteFailure::RefusedContent`] failed
/// because of these: the store and the unpushed changes were left as they
/// were.
pub fn refused_remote(dirs: &StoreDirs) -> Result<Vec<RefusedPath>, Fault> {
    let _reading = dirs.shared()?;
    let store = OpenStore::open(dirs)?;
    let repo = &store.repo;
    let (_, short) = current_branch(repo)?;
    let Ok(tip) = repo.refname_to_id(&refs::remote_tracking(&short)) else {
        return Ok(Vec::new());
    };
    let tree = repo.find_commit(tip).map_err(git_err)?.tree_id();
    Ok(guard::check_tree(repo, tree).map_err(git_err)?)
}

/// Before a change waiting to be sent is taken back: refused if a push
/// that includes it (`removed`, or a later change built on it) is on the
/// network now, as taking it back would only appear to work.
pub(crate) fn check_not_being_sent(_held: &Exclusive, store: &OpenStore, removed: git2::Oid) -> Result<(), SyncError> {
    let Some(sending) = store.dirs.lock().being_sent() else {
        return Ok(());
    };
    if sending == removed || store.repo.graph_descendant_of(sending, removed).unwrap_or(true) {
        return Err(SyncError::BeingSent);
    }
    Ok(())
}

/// Why a push did not go. Nothing was pushed.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// The remote has moved since it was fetched: fetch, integrate and try
    /// again. paw never forces.
    #[error("the remote has changes that are not in this copy")]
    NotFastForward,
    #[error(transparent)]
    Failed(Fault),
}

impl From<PawError> for PushError {
    fn from(error: PawError) -> Self {
        Self::Failed(error.into())
    }
}

/// Pushes the commit `tip` to `branch` (`refs/heads/...`) on the remote, as
/// a fast-forward only. `held` is the store lock, released for the
/// transfer; `pushing` records `tip` as on the network meanwhile. After
/// it, with the lock again, `origin/<branch>` and the last-seen ref move
/// forward to `tip` if it went, waiting up to `record_wait` for the lock
/// ([`crate::lock::RECORD_WAIT`]). The caller holds the push lock
/// throughout.
#[expect(clippy::too_many_arguments, reason = "the locks held are passed as evidence, one argument each")]
pub(crate) fn push_tip(
    connection: &Connection,
    store: &OpenStore,
    branch: &str,
    tip: git2::Oid,
    held: Exclusive,
    pushing: &crate::lock::Pushing,
    record_wait: Duration,
) -> Result<(), PushError> {
    let repo = &store.repo;
    let short = refs::short(branch).ok_or_else(|| PawError::Git("HEAD is not on a branch".into()))?;
    let sending = pushing.sending(tip);
    drop(held);

    // The commit itself, not the branch: the branch may move while this
    // runs. No leading `+`: libgit2 refuses, before sending anything, to
    // move the remote branch to a commit that does not contain its current
    // tip. Writes nothing in this repository (the remote is anonymous, so
    // there are no tracking refs for libgit2 to update).
    let refspec = format!("{tip}:{branch}");
    let mut refused = None;
    let mut behind = false;
    let sent = connection.with_remote(repo, |remote| {
        let mut callbacks = RemoteCallbacks::new();
        // A push "succeeds" even when the host rejects the branch update;
        // the rejection only shows up here.
        callbacks.push_update_reference(|_, status| {
            refused = status.map(str::to_owned);
            Ok(())
        });
        let mut options = PushOptions::new();
        options.remote_callbacks(callbacks);
        let pushed = remote.push(&[&refspec], Some(&mut options));
        behind = pushed.as_ref().is_err_and(|e| e.code() == git2::ErrorCode::NotFastForward);
        pushed
    });

    let outcome = match (sent, refused) {
        (Err(_), _) if behind => Err(PushError::NotFastForward),
        (Err(e), _) => Err(PushError::Failed(e)),
        // The host moved between our look at its branch and our update.
        (Ok(()), Some(why)) if why.contains("non-fast-forward") || why.contains("fetch first") => {
            Err(PushError::NotFastForward)
        }
        (Ok(()), Some(why)) => Err(PushError::Failed(
            RemoteFailure::PushRefused {
                detail: PawError::Git(format!("the host refused the push: {why}")).to_string(),
            }
            .into(),
        )),
        (Ok(()), None) => Ok(()),
    };
    // Not sent: the transfer's own error is what the caller needs (a
    // `NotFastForward` makes a send fetch and try again), never "busy".
    // Sent: it is on the remote whatever happens here, so recording it
    // waits long. If it still cannot, it is not an error: the next fetch
    // brings `origin/<branch>` to the remote's tip and the next integrate
    // moves last-seen.
    if outcome.is_ok()
        && let Ok(held) = store.dirs.lock().exclusive_within(record_wait)
    {
        let _ = record_push(&held, repo, short, tip);
    }
    drop(sending);
    outcome
}

/// After a push of `tip`: the remote's branch is there now. Moves
/// `origin/<short>` and the last-seen ref to it, unless another sync has
/// already moved them past it.
fn record_push(_held: &Exclusive, repo: &Repository, short: &str, tip: git2::Oid) -> Result<(), PawError> {
    for name in [refs::remote_tracking(short), refs::last_seen(short)] {
        let current = repo.refname_to_id(&name).ok();
        let further = match current {
            Some(current) if current == tip => true,
            Some(current) => repo.graph_descendant_of(current, tip).map_err(git_err)?,
            None => false,
        };
        if !further {
            repo.reference(&name, tip, true, "push").map_err(git_err)?;
        }
    }
    Ok(())
}

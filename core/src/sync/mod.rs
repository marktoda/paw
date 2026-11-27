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

use std::path::Path;
use std::time::{Duration, Instant};

use git2::{AutotagOption, Direction, FetchOptions, FileMode, PushOptions, RemoteCallbacks, Repository};

pub use history::{ChangeKind, KeptBoth};
pub use integrate::{IntegrateReport, Integration, RecipientsChange, Replaced, accept_replaced_remote};
#[cfg(feature = "sim")]
pub use integrate::integrate;
pub use transport::{ConnectError, Connection, HostKeyPin, PresentedHostKey, Remote, key_type_words, scan_host_key};

use crate::PawError;
use crate::fault::Fault;
use crate::files::refs;
use crate::lock::{Exclusive, FETCH_WAIT, Fetching, StoreLock};
use crate::remote::{RefusedPath, RemoteFailure};
use crate::path::{unsafe_path, unsafe_path_err};

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

/// Who a commit made on this device is attributed to.
pub struct Author {
    pub name: String,
    pub email: String,
}

pub(crate) fn git_err(e: git2::Error) -> PawError {
    PawError::Git(e.message().to_owned())
}

/// Starts a fetch into `repo`: takes the store's fetch lock (one fetch at a
/// time writes objects and the private refs, so two never write one pack
/// index) and clears what an earlier fetch left in the private namespace.
/// The store lock is not taken: saves and reads go on while the transfer
/// runs.
pub(crate) fn begin_fetch(store: &StoreLock, repo: &Repository) -> Result<Fetching, PawError> {
    begin_fetch_within(store, repo, FETCH_WAIT)
}

pub(crate) fn begin_fetch_within(store: &StoreLock, repo: &Repository, wait: Duration) -> Result<Fetching, PawError> {
    let fetching = store.fetching(wait)?;
    crate::lock::remove_stale_fetch_locks(store.repo());
    for name in fetched_refs(repo)?.into_iter().map(|(name, _)| name) {
        if let Ok(mut reference) = repo.find_reference(&name) {
            let _ = reference.delete();
        }
    }
    Ok(fetching)
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

/// Publishes what a fetch received. The ref updates are the critical
/// section, so the store lock is held; the fetch lock is released at the
/// end.
pub(crate) fn publish_fetched(held: &Exclusive, repo: &Repository, fetching: Fetching) -> Result<(), PawError> {
    debug_assert!(repo.workdir().is_none_or(|dir| held.covers(dir)));
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
    if let Some(dir) = repo.workdir()
        && !StoreLock::for_repo(dir).push_in_flight()
    {
        for name in pushing_refs(repo)? {
            if let Ok(mut marker) = repo.find_reference(&name) {
                let _ = marker.delete();
            }
        }
    }
    Ok(())
}

pub(crate) fn fetch_branches(
    connection: &Connection,
    store: &StoreLock,
    repo: &Repository,
    progress: Option<TransferCallback<'_>>,
) -> Result<(Option<String>, Exclusive), Fault> {
    let fetching = begin_fetch(store, repo)?;
    let default = connection.with_remote(repo, |remote| fetch_connected(remote, progress))?;
    let held = store.exclusive()?;
    publish_fetched(&held, repo, fetching)?;
    Ok((default, held))
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

/// Clones the remote's default branch into `dir`, which must be empty or
/// not exist yet. An empty repository is not an error here.
pub fn clone_with_transfer(
    connection: &Connection,
    remote: &Remote,
    dir: &Path,
    progress: Option<TransferCallback<'_>>,
) -> Result<Cloned, Fault> {
    // A new repository at `<state>/repo` is made under its store's lock;
    // one anywhere else is known to nobody until it exists.
    let repo = {
        let _held = StoreLock::for_repo(dir).exclusive()?;
        let repo = guard::init(dir)?;
        // Recorded so the copy is readable with ordinary git tools. paw itself
        // always goes through a `Connection`.
        repo.remote("origin", &remote.url()).map_err(git_err)?;
        repo
    };
    let store = StoreLock::for_repo(dir);

    let (branch, held) = fetch_branches(connection, &store, &repo, progress)?;
    let Some(branch) = branch else {
        return Ok(Cloned::Empty);
    };
    let short = refs::short(&branch).ok_or_else(|| PawError::Git(format!("unexpected default branch {branch}")))?;
    let tip = repo.refname_to_id(&refs::remote_tracking(short)).map_err(git_err)?;
    // Before anything is checked out, or the branch made.
    let tree = repo.find_commit(tip).map_err(git_err)?.tree_id();
    let refused = guard::check_tree(&repo, tree).map_err(git_err)?;
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
pub fn refused_remote(dir: &Path) -> Result<Vec<RefusedPath>, Fault> {
    let _reading = StoreLock::for_repo(dir).shared()?;
    let repo = guard::open(dir)?;
    let (_, short) = current_branch(&repo)?;
    let Ok(tip) = repo.refname_to_id(&refs::remote_tracking(&short)) else {
        return Ok(Vec::new());
    };
    let tree = repo.find_commit(tip).map_err(git_err)?.tree_id();
    Ok(guard::check_tree(&repo, tree).map_err(git_err)?)
}

/// Commits `contents` as the file `path` (relative to the store, `/`
/// separated) on the current branch, and nothing else.
///
/// The commit is built from the branch's tree, not from the working copy,
/// and the file reaches the working copy through libgit2's checkout. A
/// cloned repository is not trusted and may contain symbolic links, so a
/// path that runs through one, in the tree or on disk, is refused with
/// [`PawError::UnsafePath`] (or [`Fault::Refused`], for a name the tree
/// refuses) before anything is written.
///
/// `message` is the subject line, in `pass`'s own wording
/// ("Add given password for web/example to store.") so the history reads the
/// same whichever tool wrote it.
pub fn commit(
    dir: &Path,
    path: &str,
    contents: &[u8],
    message: &str,
    author: &Author,
) -> Result<String, Fault> {
    let held = StoreLock::for_repo(dir).exclusive()?;
    commit_edits(&held, dir, &[TreeEdit::Write { path, contents }], message, author)
}

/// One change to one file in a commit made by [`commit_edits`].
pub(crate) enum TreeEdit<'a> {
    Write { path: &'a str, contents: &'a [u8] },
    /// Remove the file `path`, which must be a regular file in the tree.
    Remove { path: &'a str },
    /// Move the file `from` to `to` unchanged: the same blob, so git and
    /// the replay see a rename.
    Rename { from: &'a str, to: &'a str },
}

impl TreeEdit<'_> {
    fn paths(&self) -> Vec<&str> {
        match self {
            Self::Write { path, .. } | Self::Remove { path } => vec![path],
            Self::Rename { from, to } => vec![from, to],
        }
    }
}

/// Checks `path` in the working copy before anything is written: no link
/// on the way and no file where a folder should be. The working copy can
/// differ from the tree, and checkout does follow a folder link it finds
/// there (seen with libgit2 1.9). `symlink_metadata` does not follow links.
fn check_disk_path(dir: &Path, path: &str) -> Result<(), PawError> {
    let mut on_disk = dir.to_path_buf();
    let mut components = path.split('/').peekable();
    while let Some(component) = components.next() {
        on_disk.push(component);
        let is_folder = components.peek().is_some();
        match std::fs::symlink_metadata(&on_disk) {
            Ok(found) if found.is_symlink() || (is_folder && !found.is_dir()) => {
                let why = "a link or a file in the working copy is in its way";
                return Err(unsafe_path_err(path, why));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Commits `edits` on the current branch as one commit, and changes
/// nothing else; see [`commit`] for the protections.
/// `held` is the store's lock: the whole commit, from reading `HEAD` to the
/// checkout, is one critical section.
pub(crate) fn commit_edits(
    held: &Exclusive,
    dir: &Path,
    edits: &[TreeEdit<'_>],
    message: &str,
    author: &Author,
) -> Result<String, Fault> {
    debug_assert!(held.covers(dir), "commit_edits without its store's lock");
    for path in edits.iter().flat_map(TreeEdit::paths) {
        if let Some(why) = unsafe_path(path) {
            return Err(unsafe_path_err(path, why).into());
        }
    }
    let repo = guard::open(dir)?;
    let parent = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(git_err)?;
    let base = parent.tree().map_err(git_err)?;

    for path in edits.iter().flat_map(TreeEdit::paths) {
        history::check_tree_path(&base, path)?;
        check_disk_path(dir, path)?;
    }

    let mut index = git2::Index::new().map_err(git_err)?;
    index.read_tree(&base).map_err(git_err)?;
    for edit in edits {
        let existing = |path: &str| {
            base.get_path(Path::new(path))
                .map(|entry| entry.id())
                .map_err(|_| PawError::Store(format!("there is no file called {path}")))
        };
        match edit {
            TreeEdit::Write { path, contents } => {
                let blob = repo.blob(contents).map_err(git_err)?;
                index
                    .add(&history::index_entry(path, blob, FileMode::Blob.into()))
                    .map_err(git_err)?;
            }
            TreeEdit::Remove { path } => {
                existing(path)?;
                index.remove_path(Path::new(path)).map_err(git_err)?;
            }
            TreeEdit::Rename { from, to } => {
                let blob = existing(from)?;
                index.remove_path(Path::new(from)).map_err(git_err)?;
                index
                    .add(&history::index_entry(to, blob, FileMode::Blob.into()))
                    .map_err(git_err)?;
            }
        }
    }
    let tree_id = index.write_tree_to(&repo).map_err(git_err)?;
    // A new name can collide with one already there (`Web/x` beside `web/`).
    let refused = guard::check_tree(&repo, tree_id).map_err(git_err)?;
    if !refused.is_empty() {
        return Err(Fault::Refused { paths: refused });
    }
    let tree = repo.find_tree(tree_id).map_err(git_err)?;

    let signature = git2::Signature::now(&author.name, &author.email).map_err(git_err)?;
    let id = repo
        .commit(Some("HEAD"), &signature, &signature, message, &tree, &[&parent])
        .map_err(git_err)?;
    // The names are taken literally. Left to itself, checkout reads them as
    // patterns, and an entry called `web/*` would overwrite its neighbours.
    let mut only_these_files = guard::checkout();
    only_these_files.disable_pathspec_match(true);
    for path in edits.iter().flat_map(TreeEdit::paths) {
        only_these_files.path(path);
    }
    repo.checkout_head(Some(&mut only_these_files)).map_err(git_err)?;
    Ok(id.to_string())
}

/// Every push record (`refs/paw/pushing/*`).
fn pushing_refs(repo: &Repository) -> Result<Vec<String>, PawError> {
    let mut names = Vec::new();
    for reference in repo.references().map_err(git_err)? {
        let reference = reference.map_err(git_err)?;
        if let Ok(name) = reference.name()
            && refs::is_pushing(name)
        {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

/// Before a change waiting to be sent is taken back: refused if a push
/// that includes it (`removed`, or a later change built on it) is on the
/// network now, as taking it back would only appear to work. A record
/// left by a push whose process died is removed.
pub(crate) fn check_not_being_sent(
    _held: &Exclusive,
    store: &StoreLock,
    repo: &Repository,
    short: &str,
    removed: git2::Oid,
) -> Result<(), SyncError> {
    let name = refs::pushing(short);
    let Ok(sending) = repo.refname_to_id(&name) else {
        return Ok(());
    };
    if !store.push_in_flight() {
        if let Ok(mut stale) = repo.find_reference(&name) {
            let _ = stale.delete();
        }
        return Ok(());
    }
    if sending == removed || repo.graph_descendant_of(sending, removed).unwrap_or(true) {
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
/// a fast-forward only. `held` is the store lock: it is used to record
/// which commit is going out, then released for the transfer. After it,
/// with the lock again, `origin/<branch>` and the last-seen ref move
/// forward to `tip` if it went, waiting up to `record_wait` for the lock
/// ([`crate::lock::RECORD_WAIT`]). The caller holds the push lock
/// throughout.
#[expect(clippy::too_many_arguments, reason = "the locks held are passed as evidence, one argument each")]
pub(crate) fn push_tip(
    connection: &Connection,
    store: &StoreLock,
    repo: &Repository,
    branch: &str,
    tip: git2::Oid,
    held: Exclusive,
    _pushing: &crate::lock::Pushing,
    record_wait: Duration,
) -> Result<(), PushError> {
    let short = refs::short(branch).ok_or_else(|| PawError::Git("HEAD is not on a branch".into()))?;
    repo.reference(&refs::pushing(short), tip, true, "push").map_err(git_err)?;
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
    let clear_marker = || {
        if let Ok(mut marker) = repo.find_reference(&refs::pushing(short)) {
            let _ = marker.delete();
        }
    };
    match outcome {
        // Not sent: the transfer's own error is what the caller needs (a
        // `NotFastForward` makes a send fetch and try again), never "busy".
        // The marker goes if the lock comes quickly; if not, the next
        // discard finds no push in flight and removes it.
        Err(error) => {
            if let Ok(_held) = store.exclusive_within(Duration::from_secs(2)) {
                clear_marker();
            }
            Err(error)
        }
        // Sent: it is on the remote whatever happens here, so recording it
        // waits much longer. If it still cannot, it is not an error: the
        // next fetch brings `origin/<branch>` to the remote's tip and the
        // next integrate moves last-seen, and the marker is removed as
        // stale once no push is in flight.
        Ok(()) => {
            if let Ok(held) = store.exclusive_within(record_wait) {
                let _ = record_push(&held, repo, short, tip);
                clear_marker();
            }
            Ok(())
        }
    }
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

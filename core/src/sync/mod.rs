//! Keeping the local copy of the password store in step with the git host.
//!
//! This is the whole of paw's use of libgit2: [`clone`], [`fetch`],
//! [`integrate`], [`commit`] and [`push`], plus the unpushed [`queue`]. The
//! network ones take a [`Connection`], which the caller opens once and may
//! use for several of them; see `transport` for why that matters.
//! [`operation`] runs a whole sync on one connection, so one login.
//!
//! A push only ever fast-forwards. If the remote has moved, [`push`] fails
//! with [`PawError::NotFastForward`] and changes nothing.

pub(crate) mod guard;
mod history;
pub mod hosts;
pub mod integrate;
pub mod operation;
pub mod queue;
mod transport;

use std::path::Path;
use std::time::{Duration, Instant};

use git2::{AutotagOption, Direction, FetchOptions, FileMode, PushOptions, RemoteCallbacks, Repository};

pub(crate) use guard::Refused;
pub use guard::RefusedPath;
pub use history::{ChangeKind, KeptBoth};
pub use integrate::{
    IntegrateReport, Integration, RecipientsChange, Replaced, accept_replaced_remote, integrate,
};
pub use transport::{Connection, DOWNLOAD_LIMIT, HostKeyPin, PresentedHostKey, Remote, scan_host_key};
pub(crate) use transport::made_no_connection;
#[cfg(feature = "sim")]
pub(crate) use transport::redirect;

use crate::PawError;
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
    /// The host presented a key other than the pinned ones. Nothing was sent.
    #[error("{host}'s key changed: expected {}, it presented {presented}", .expected.join(" or "))]
    HostKeyChanged { host: String, expected: Vec<String>, presented: String },
    /// The host does not accept the key. Asked before the PIN or a touch.
    #[error("the git host did not accept this key")]
    KeyRejected,
    #[error("wrong PIN, {tries_left} tries left")]
    WrongPin { tries_left: u8 },
    #[error("the PIN is blocked; unblock it with the admin PIN")]
    PinBlocked,
    /// The key did not sign: most often nobody touched it in time.
    #[error("the key did not sign the login: {message}")]
    Sign { message: String },
    #[error("could not reach the git host: {message}")]
    Network { message: String },
    /// The remote moved again while pushing, after one retry.
    #[error("the remote kept changing; try again")]
    RemoteKeptMoving,
    #[error("{message}")]
    Failed { message: String },
}

impl From<PawError> for SyncError {
    fn from(error: PawError) -> Self {
        match error {
            PawError::KeyRejected => Self::KeyRejected,
            PawError::WrongPin { tries_left } => Self::WrongPin { tries_left },
            PawError::PinBlocked => Self::PinBlocked,
            PawError::Sign(message) => Self::Sign { message },
            PawError::Network(message) => Self::Network { message },
            other => Self::Failed { message: other.to_string() },
        }
    }
}

/// How far a clone or fetch has got, for a progress bar: objects received
/// and expected. Return `false` to stop.
pub type Progress<'a> = &'a mut dyn FnMut(u32, u32) -> bool;

/// [`Progress`] with the bytes received too.
pub type TransferCallback<'a> = &'a mut dyn FnMut(TransferProgress) -> bool;

/// How far a clone or fetch has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct TransferProgress {
    /// Objects received so far.
    pub received_objects: u32,
    /// Objects the host is sending in all; 0 until it has said.
    pub total_objects: u32,
    /// Bytes of the pack received so far.
    pub received_bytes: u64,
}

/// The shortest time between two progress reports: at most ten a second.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Decides which of libgit2's progress updates (one per object, and more
/// while it indexes) are reported: the first, then at most one per
/// [`PROGRESS_INTERVAL`], plus the moment the last object arrives, so a
/// bar does not stop short of the end. A repeat of the last report is never
/// sent, and once [`Self::stop`] is called (the listener said stop, or the
/// work was cancelled) nothing is.
#[derive(Debug, Clone)]
pub struct ProgressThrottle {
    interval: Duration,
    last: Option<(Instant, TransferProgress)>,
    stopped: bool,
}

impl ProgressThrottle {
    pub fn new(interval: Duration) -> Self {
        Self { interval, last: None, stopped: false }
    }

    /// Whether to report `update`, arriving at `now`. Records it if so.
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
            Some((at, _)) => now.saturating_duration_since(at) >= self.interval,
        };
        if report {
            self.last = Some((now, update));
        }
        report
    }

    /// Nothing more is reported.
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
    let mut throttle = ProgressThrottle::new(PROGRESS_INTERVAL);
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

/// Every branch of the remote is mirrored under `origin/`.
const FETCH_REFSPEC: &str = "+refs/heads/*:refs/remotes/origin/*";

/// Who a commit made on this device is attributed to.
pub struct Author {
    pub name: String,
    pub email: String,
}

pub(crate) fn git_err(e: git2::Error) -> PawError {
    PawError::Git(e.message().to_owned())
}

/// Fetches all branches and returns the remote's default branch
/// (`refs/heads/...`), if it has one.
fn fetch_branches(
    connection: &Connection,
    repo: &Repository,
    progress: Option<TransferCallback<'_>>,
) -> Result<Option<String>, PawError> {
    connection.with_remote(repo, |remote| fetch_connected(remote, progress))
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
    if let Some(progress) = progress {
        let mut callbacks = RemoteCallbacks::new();
        callbacks.transfer_progress(throttled(progress));
        options.remote_callbacks(callbacks);
    }
    connected
        .remote()
        .fetch(&[FETCH_REFSPEC], Some(&mut options), None)?;
    Ok(default)
}

/// The branch `HEAD` points at, as `(refs/heads/<name>, <name>)`.
pub(crate) fn current_branch(repo: &Repository) -> Result<(String, String), PawError> {
    let head = repo.find_reference("HEAD").map_err(git_err)?;
    let not_on_branch = || PawError::Git("HEAD is not on a branch".into());
    let full = head.symbolic_target().map_err(git_err)?.ok_or_else(not_on_branch)?;
    let short = full.strip_prefix("refs/heads/").ok_or_else(not_on_branch)?;
    Ok((full.to_owned(), short.to_owned()))
}

/// Clones the remote's default branch into `dir`, which must be empty or
/// not exist yet.
pub fn clone(connection: &Connection, remote: &Remote, dir: &Path) -> Result<(), PawError> {
    match clone_with_progress(connection, remote, dir, None)? {
        Cloned::Store => Ok(()),
        Cloned::Empty => Err(PawError::Git("the remote repository is empty".into())),
        Cloned::Refused(paths) => Err(guard::Refused(paths).into()),
    }
}

/// [`clone`], reporting objects received and expected to `progress`, which
/// returns `false` to stop. An empty repository is not an error here.
/// Reports are throttled as [`ProgressThrottle`] says.
pub fn clone_with_progress(
    connection: &Connection,
    remote: &Remote,
    dir: &Path,
    progress: Option<Progress<'_>>,
) -> Result<Cloned, PawError> {
    match progress {
        Some(progress) => {
            let mut report = |update: TransferProgress| progress(update.received_objects, update.total_objects);
            clone_with_transfer(connection, remote, dir, Some(&mut report))
        }
        None => clone_with_transfer(connection, remote, dir, None),
    }
}

/// [`clone_with_progress`], reporting bytes too.
pub fn clone_with_transfer(
    connection: &Connection,
    remote: &Remote,
    dir: &Path,
    progress: Option<TransferCallback<'_>>,
) -> Result<Cloned, PawError> {
    let repo = guard::init(dir)?;
    // Recorded so the copy is readable with ordinary git tools. paw itself
    // always goes through a `Connection`.
    repo.remote("origin", &remote.url()).map_err(git_err)?;

    let Some(branch) = fetch_branches(connection, &repo, progress)? else {
        return Ok(Cloned::Empty);
    };
    let short = branch
        .strip_prefix("refs/heads/")
        .ok_or_else(|| PawError::Git(format!("unexpected default branch {branch}")))?;
    let tip = repo
        .refname_to_id(&format!("refs/remotes/origin/{short}"))
        .map_err(git_err)?;
    // Before anything is checked out, or the branch made.
    let tree = repo.find_commit(tip).map_err(git_err)?.tree_id();
    let refused = guard::check_tree(&repo, tree).map_err(git_err)?;
    if !refused.is_empty() {
        return Ok(Cloned::Refused(refused));
    }

    repo.reference(&branch, tip, false, "clone").map_err(git_err)?;
    repo.reference(&history::last_seen_ref(short), tip, true, "clone")
        .map_err(git_err)?;
    repo.set_head(&branch).map_err(git_err)?;
    repo.checkout_head(Some(&mut guard::checkout())).map_err(git_err)?;
    Ok(Cloned::Store)
}

/// What the fetched tip of the current branch's remote counterpart holds
/// that paw will not check out; empty if nothing (or nothing was fetched).
/// A sync that failed with a message starting "the remote contains
/// something paw will not check out" failed because of these: the store
/// and the unpushed changes were left as they were.
pub fn refused_remote(dir: &Path) -> Result<Vec<RefusedPath>, SyncError> {
    let repo = guard::open(dir)?;
    let (_, short) = current_branch(&repo)?;
    let Ok(tip) = repo.refname_to_id(&format!("refs/remotes/origin/{short}")) else {
        return Ok(Vec::new());
    };
    let tree = repo.find_commit(tip).map_err(git_err)?.tree_id();
    Ok(guard::check_tree(&repo, tree).map_err(git_err)?)
}

/// Updates `origin/*` from the remote and returns the new tip of the current
/// branch's remote counterpart. The working copy is not changed.
pub fn fetch(connection: &Connection, dir: &Path) -> Result<String, PawError> {
    fetch_with_transfer(connection, dir, None)
}

/// [`fetch`], reporting how far it has got to `progress` (throttled), which
/// returns `false` to stop.
pub fn fetch_with_transfer(
    connection: &Connection,
    dir: &Path,
    progress: Option<TransferCallback<'_>>,
) -> Result<String, PawError> {
    let repo = guard::open(dir)?;
    fetch_branches(connection, &repo, progress)?;
    let (_, short) = current_branch(&repo)?;
    repo.refname_to_id(&format!("refs/remotes/origin/{short}"))
        .map(|id| id.to_string())
        .map_err(git_err)
}

/// Commits `contents` as the file `path` (relative to the store, `/`
/// separated) on the current branch, and nothing else. Returns the commit id.
///
/// The commit is built from the branch's tree, not from the working copy,
/// and the file reaches the working copy through libgit2's checkout. A
/// cloned repository is not trusted and may contain symbolic links, so a
/// path that runs through one, in the tree or on disk, is refused with
/// [`PawError::UnsafePath`] before anything is written.
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
) -> Result<String, PawError> {
    commit_edits(dir, &[TreeEdit::Write { path, contents }], message, author)
}

/// One change to one file in a commit made by [`commit_edits`].
pub(crate) enum TreeEdit<'a> {
    /// Add or replace the file `path` with `contents`.
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
/// nothing else; see [`commit`] for the protections. Returns the commit id.
pub(crate) fn commit_edits(
    dir: &Path,
    edits: &[TreeEdit<'_>],
    message: &str,
    author: &Author,
) -> Result<String, PawError> {
    for path in edits.iter().flat_map(TreeEdit::paths) {
        if let Some(why) = unsafe_path(path) {
            return Err(unsafe_path_err(path, why));
        }
    }
    let repo = guard::open(dir)?;
    let parent = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(git_err)?;
    let base = parent.tree().map_err(git_err)?;

    // Every folder on the way must be a real folder in the tree (or not
    // exist yet), and the entry itself a regular file (or not exist yet).
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
        return Err(guard::Refused(refused).into());
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

/// Pushes the current branch to the branch of the same name, as a
/// fast-forward only.
pub fn push(connection: &Connection, dir: &Path) -> Result<(), PawError> {
    let repo = guard::open(dir)?;
    let (branch, short) = current_branch(&repo)?;
    let tip = repo.refname_to_id(&branch).map_err(git_err)?;

    // No leading `+`: libgit2 refuses, before sending anything, to move the
    // remote branch to a commit that does not contain its current tip.
    let refspec = format!("{branch}:{branch}");
    let mut refused = None;
    connection.with_remote(&repo, |remote| {
        let mut callbacks = RemoteCallbacks::new();
        // A push "succeeds" even when the host rejects the branch update;
        // the rejection only shows up here.
        callbacks.push_update_reference(|_, status| {
            refused = status.map(str::to_owned);
            Ok(())
        });
        let mut options = PushOptions::new();
        options.remote_callbacks(callbacks);
        remote.push(&[&refspec], Some(&mut options))
    })?;

    match refused {
        // The host moved between our look at its branch and our update.
        Some(why) if why.contains("non-fast-forward") || why.contains("fetch first") => {
            Err(PawError::NotFastForward)
        }
        Some(why) => Err(PawError::Git(format!("the host refused the push: {why}"))),
        None => {
            let tracking = format!("refs/remotes/origin/{short}");
            repo.reference(&tracking, tip, true, "push").map_err(git_err)?;
            repo.reference(&history::last_seen_ref(&short), tip, true, "push")
                .map_err(git_err)?;
            Ok(())
        }
    }
}

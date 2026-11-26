//! The "not yet pushed" queue: paw's local commits that the remote does not
//! have, as the person sees them, and taking some or all of them back.
//!
//! Only commits after the remote-tracking tip are ever rewritten. Pushed
//! history is never touched.

use std::path::Path;

use git2::{Oid, Repository};

use super::history::{ChangeKind, PawCommit, Replay, ensure_clean, move_branch, paw_commits};
use super::{SyncError, check_not_being_sent, current_branch, git_err};
use crate::files::refs;
use crate::lock::{Exclusive, StoreLock};

/// One unpushed change.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct QueueItem {
    /// Pass this to [`discard`].
    pub id: String,
    pub kind: ChangeKind,
    /// The entry, e.g. `web/github.com`; for a rename, its new name.
    pub entry: String,
    /// For a rename, the name it had before.
    pub previous: Option<String>,
    /// When the change was made, in seconds since 1970 (the commit's author
    /// time).
    pub when: i64,
}

/// The changes waiting to be sent, on the current branch.
pub(crate) struct Waiting<'r> {
    pub(crate) branch: String,
    pub(crate) short: String,
    pub(crate) head: Oid,
    /// Oldest first.
    pub(crate) commits: Vec<PawCommit<'r>>,
}

/// The changes waiting to be sent: this device's commits that neither the
/// remote's fetched tip nor the tip paw last saw of it contains. Each is
/// one change paw made; a commit another tool made is refused
/// ([`SyncError::NotPawHistory`]). A store with no remote has none. This is
/// what the queue lists, a send pushes and every count of waiting changes
/// counts.
pub(crate) fn waiting(repo: &Repository) -> Result<Waiting<'_>, SyncError> {
    let (branch, short) = current_branch(repo)?;
    let head = repo.refname_to_id(&branch).map_err(git_err)?;
    let mut pushed = Vec::new();
    for name in [refs::remote_tracking(&short), refs::last_seen(&short)] {
        if let Ok(id) = repo.refname_to_id(&name) {
            pushed.push(id);
        }
    }
    let commits = if pushed.is_empty() { Vec::new() } else { paw_commits(repo, head, &pushed)? };
    Ok(Waiting { branch, short, head, commits })
}

fn item(commit: &PawCommit<'_>) -> QueueItem {
    QueueItem {
        id: commit.commit.id().to_string(),
        kind: commit.kind,
        entry: commit.entry(),
        previous: commit.previous(),
        when: commit.commit.author().when().seconds(),
    }
}

/// The unpushed changes, newest first.
pub fn unpushed(dir: &Path) -> Result<Vec<QueueItem>, SyncError> {
    let _reading = StoreLock::for_repo(dir).shared()?;
    let repo = super::guard::open(dir)?;
    let local = waiting(&repo)?;
    Ok(local.commits.iter().rev().map(item).collect())
}

/// Takes back the unpushed change `id`. Later changes are kept, rebuilt
/// without it. Returns the queue afterwards.
pub fn discard(dir: &Path, id: &str) -> Result<Vec<QueueItem>, SyncError> {
    let held = StoreLock::for_repo(dir).exclusive()?;
    discard_locked(&held, dir, id)?;
    unpushed(dir)
}

fn discard_locked(held: &Exclusive, dir: &Path, id: &str) -> Result<(), SyncError> {
    let repo = super::guard::open(dir)?;
    let local = waiting(&repo)?;
    let Some(position) = local.commits.iter().position(|c| c.commit.id().to_string() == id) else {
        return Err(SyncError::NotWaiting { id: id.to_owned() });
    };
    check_not_being_sent(held, &StoreLock::for_repo(dir), &repo, &local.short, local.commits[position].commit.id())?;
    ensure_clean(&repo)?;
    let base = local.commits[position].commit.parent_id(0).map_err(git_err)?;
    let mut replay = Replay::onto(&repo, base)?;
    for later in &local.commits[position + 1..] {
        replay.apply(later)?;
    }
    move_branch(held, &repo, &local.branch, local.head, replay.tip(), None, "paw: discard a change")
}

/// Takes back the most recent unpushed change. Returns the queue afterwards.
pub fn undo_last(dir: &Path) -> Result<Vec<QueueItem>, SyncError> {
    // One critical section: the newest change is the one taken back, even
    // if another save is waiting to add a newer one.
    let held = StoreLock::for_repo(dir).exclusive()?;
    let newest = unpushed(dir)?.into_iter().next();
    if let Some(item) = newest {
        discard_locked(&held, dir, &item.id)?;
    }
    unpushed(dir)
}

/// Takes back every unpushed change: the store goes back to what was last
/// fetched from the remote.
pub fn discard_all(dir: &Path) -> Result<(), SyncError> {
    let held = StoreLock::for_repo(dir).exclusive()?;
    let repo = super::guard::open(dir)?;
    let local = waiting(&repo)?;
    let Some(oldest) = local.commits.first() else {
        return Ok(());
    };
    check_not_being_sent(&held, &StoreLock::for_repo(dir), &repo, &local.short, oldest.commit.id())?;
    ensure_clean(&repo)?;
    let base = oldest.commit.parent_id(0).map_err(git_err)?;
    move_branch(&held, &repo, &local.branch, local.head, base, None, "paw: discard all changes")
}

//! The "not yet pushed" queue: paw's local commits that the remote does not
//! have, as the person sees them, and taking some or all of them back.
//!
//! Only commits after the remote-tracking tip are ever rewritten. Pushed
//! history is never touched.

use std::path::Path;

use git2::{Oid, Repository};

use super::history::{
    ChangeKind, PawCommit, Replay, ensure_clean, last_seen_ref, move_branch, paw_commits,
};
use super::{SyncError, current_branch, git_err};

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

struct Local<'r> {
    branch: String,
    head: Oid,
    commits: Vec<PawCommit<'r>>,
}

fn local(repo: &Repository) -> Result<Local<'_>, SyncError> {
    let (branch, short) = current_branch(repo)?;
    let head = repo.refname_to_id(&branch).map_err(git_err)?;
    let mut pushed = Vec::new();
    for name in [format!("refs/remotes/origin/{short}"), last_seen_ref(&short)] {
        if let Ok(id) = repo.refname_to_id(&name) {
            pushed.push(id);
        }
    }
    // A store with no remote has nothing it could push.
    let commits = if pushed.is_empty() { Vec::new() } else { paw_commits(repo, head, &pushed)? };
    Ok(Local { branch, head, commits })
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
    let repo = super::guard::open(dir)?;
    let local = local(&repo)?;
    Ok(local.commits.iter().rev().map(item).collect())
}

/// Takes back the unpushed change `id`. Later changes are kept, rebuilt
/// without it. Returns the queue afterwards.
pub fn discard(dir: &Path, id: &str) -> Result<Vec<QueueItem>, SyncError> {
    let repo = super::guard::open(dir)?;
    let local = local(&repo)?;
    let Some(position) = local.commits.iter().position(|c| c.commit.id().to_string() == id) else {
        return Err(SyncError::Failed { message: format!("{id} is not an unpushed change") });
    };
    ensure_clean(&repo)?;
    let base = local.commits[position].commit.parent_id(0).map_err(git_err)?;
    let mut replay = Replay::onto(&repo, base)?;
    for later in &local.commits[position + 1..] {
        replay.apply(later)?;
    }
    move_branch(&repo, &local.branch, local.head, replay.tip(), None, "paw: discard a change")?;
    drop(local);
    unpushed(dir)
}

/// Takes back the most recent unpushed change. Returns the queue afterwards.
pub fn undo_last(dir: &Path) -> Result<Vec<QueueItem>, SyncError> {
    let newest = unpushed(dir)?.into_iter().next();
    match newest {
        Some(item) => discard(dir, &item.id),
        None => Ok(Vec::new()),
    }
}

/// Takes back every unpushed change: the store goes back to what was last
/// fetched from the remote.
pub fn discard_all(dir: &Path) -> Result<(), SyncError> {
    let repo = super::guard::open(dir)?;
    let local = local(&repo)?;
    let Some(oldest) = local.commits.first() else {
        return Ok(());
    };
    ensure_clean(&repo)?;
    let base = oldest.commit.parent_id(0).map_err(git_err)?;
    move_branch(&repo, &local.branch, local.head, base, None, "paw: discard all changes")
}

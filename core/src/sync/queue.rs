//! The "not yet pushed" queue: paw's local commits that the remote does not
//! have, as the person sees them, and taking some or all of them back.
//!
//! Only commits after the remote-tracking tip are ever rewritten. Pushed
//! history is never touched.

use std::collections::{HashMap, HashSet};
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

/// Rewrites the waiting changes from the one at `from` on, and moves the
/// branch once: refused if that change, or one built on it, is being sent,
/// or if the working copy is not clean. `rebuild` makes the new tip from
/// the parent of the change at `from`. Every rewrite of the queue goes
/// through here, under `held`.
fn rewrite_from(
    held: &Exclusive,
    dir: &Path,
    repo: &Repository,
    local: &Waiting<'_>,
    from: usize,
    why: &str,
    rebuild: impl FnOnce(Oid) -> Result<Oid, SyncError>,
) -> Result<(), SyncError> {
    check_not_being_sent(held, &StoreLock::for_repo(dir), repo, &local.short, local.commits[from].commit.id())?;
    ensure_clean(repo)?;
    let base = local.commits[from].commit.parent_id(0).map_err(git_err)?;
    let tip = rebuild(base)?;
    move_branch(held, repo, &local.branch, local.head, tip, None, why)
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
    rewrite_from(held, dir, &repo, &local, position, "paw: discard a change", |base| {
        let mut replay = Replay::onto(&repo, base)?;
        for later in &local.commits[position + 1..] {
            replay.apply(later)?;
        }
        Ok(replay.tip())
    })
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
    if local.commits.is_empty() {
        return Ok(());
    }
    rewrite_from(&held, dir, &repo, &local, 0, "paw: discard all changes", Ok)
}

/// A waiting change that leaves a file (an add, an edit or a rename), with
/// the ciphertext it would send.
pub(crate) struct WaitingFile {
    /// As in [`QueueItem::id`].
    pub id: String,
    pub entry: String,
    pub path: String,
    pub blob: Oid,
    pub ciphertext: Vec<u8>,
}

/// The waiting changes that leave a file, oldest first, and the queue's
/// head as read, for [`rewrite_ciphertexts`]. Call it holding the store's
/// lock.
pub(crate) fn waiting_files(dir: &Path) -> Result<(Oid, Vec<WaitingFile>), SyncError> {
    let repo = super::guard::open(dir)?;
    let local = waiting(&repo)?;
    let mut files = Vec::new();
    for commit in &local.commits {
        let Some((path, blob)) = commit.change.ciphertext() else { continue };
        files.push(WaitingFile {
            id: commit.commit.id().to_string(),
            entry: commit.entry(),
            path: path.to_owned(),
            blob,
            ciphertext: repo.find_blob(blob).map_err(git_err)?.content().to_vec(),
        });
    }
    Ok((local.head, files))
}

/// Gives a waiting change's new ciphertext from its path and blob, or
/// `None` to keep it; for [`rewrite_ciphertexts`].
pub(crate) type Replace<'a> = &'a mut dyn FnMut(&str, Oid) -> Result<Option<Vec<u8>>, SyncError>;

/// What [`rewrite_ciphertexts`] did.
pub(crate) enum Rewritten {
    Done,
    /// The queue changed since its head was read: nothing was done.
    Moved,
    /// Every ciphertext was kept: nothing was done.
    Unchanged,
}

/// Replaces waiting changes' ciphertexts in place, keeping the same changes
/// in the same order with the same messages and times. `replace` is asked
/// once for each (path, ciphertext) the waiting changes leave, oldest
/// first, and gives the new ciphertext, or `None` to keep it. Only if the
/// queue is still at `head`; under the store's lock throughout.
pub(crate) fn rewrite_ciphertexts(
    dir: &Path,
    head: Oid,
    replace: Replace<'_>,
) -> Result<Rewritten, SyncError> {
    let held = StoreLock::for_repo(dir).exclusive()?;
    let repo = super::guard::open(dir)?;
    let local = waiting(&repo)?;
    if local.head != head {
        return Ok(Rewritten::Moved);
    }
    let mut asked = HashSet::new();
    let mut swaps = HashMap::new();
    let mut first = None;
    for (at, commit) in local.commits.iter().enumerate() {
        let Some((path, blob)) = commit.change.ciphertext() else { continue };
        let key = (path.to_owned(), blob);
        if asked.insert(key.clone())
            && let Some(ciphertext) = replace(path, blob)?
        {
            swaps.insert(key.clone(), repo.blob(&ciphertext).map_err(git_err)?);
        }
        if swaps.contains_key(&key) {
            first.get_or_insert(at);
        }
    }
    let Some(first) = first else { return Ok(Rewritten::Unchanged) };
    rewrite_from(&held, dir, &repo, &local, first, "paw: re-encrypt waiting changes", |base| {
        let mut replay = Replay::onto(&repo, base)?.swapping(swaps);
        for commit in &local.commits[first..] {
            replay.apply(commit)?;
        }
        Ok(replay.tip())
    })?;
    Ok(Rewritten::Done)
}

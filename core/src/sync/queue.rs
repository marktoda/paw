//! The "not yet pushed" queue: paw's local commits that the remote does not
//! have, as the person sees them, and taking some or all of them back.
//!
//! Only commits after the remote-tracking tip are ever rewritten. Pushed
//! history is never touched.

use std::path::Path;

use git2::{Oid, Repository};

use super::history::{
    Change, ChangeKind, PawCommit, Replay, ensure_clean, last_seen_ref, move_branch, paw_commits,
};
use super::{SyncError, current_branch, git_err};
use crate::lock::{Exclusive, StoreLock};

/// [`super::check_not_being_sent`] for the branch `refs/heads/<short>`.
fn check_not_being_sent(
    held: &Exclusive,
    store: &StoreLock,
    repo: &Repository,
    branch: &str,
    removed: Oid,
) -> Result<(), SyncError> {
    let short = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    super::check_not_being_sent(held, store, repo, short, removed)
}

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
    let _reading = StoreLock::for_repo(dir).shared()?;
    let repo = super::guard::open(dir)?;
    let local = local(&repo)?;
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
    let local = local(&repo)?;
    let Some(position) = local.commits.iter().position(|c| c.commit.id().to_string() == id) else {
        return Err(SyncError::Failed { message: format!("{id} is not an unpushed change") });
    };
    check_not_being_sent(held, &StoreLock::for_repo(dir), &repo, &local.branch, local.commits[position].commit.id())?;
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
    let local = local(&repo)?;
    let Some(oldest) = local.commits.first() else {
        return Ok(());
    };
    check_not_being_sent(&held, &StoreLock::for_repo(dir), &repo, &local.branch, oldest.commit.id())?;
    ensure_clean(&repo)?;
    let base = oldest.commit.parent_id(0).map_err(git_err)?;
    move_branch(&held, &repo, &local.branch, local.head, base, None, "paw: discard all changes")
}

/// A waiting change whose ciphertext is not encrypted for exactly the
/// recipients its entry has now: its `.gpg-id` changed after it was saved
/// (a pull brought a new one, or it was moved), or the new recipients are
/// not accepted yet. paw does not push it: the ciphertext would reach the
/// remote, and its history, encrypted for the wrong people.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HeldBack {
    /// The waiting change, as in [`QueueItem::id`].
    pub id: String,
    pub entry: String,
    /// The `.gpg-id` that governs the entry now, e.g. `work/.gpg-id`; empty
    /// if none does.
    pub gpg_id: String,
    /// Who the waiting ciphertext is encrypted for, as recipients are
    /// described elsewhere ("key ID 0123...").
    pub encrypted_for: Vec<String>,
    /// The key lines of that `.gpg-id` now.
    pub recipients: Vec<String>,
    /// The person has accepted those recipients on this device. Until they
    /// have, paw cannot encrypt to them either.
    pub accepted: bool,
}

/// Whether `found` names exactly the keys of `keys`, no more, no fewer,
/// and nothing hidden or passphrase-only.
fn encrypted_for_exactly(found: &crate::recipients::EntryRecipients, keys: &[pgp::packet::PublicKey]) -> bool {
    use crate::recipients::Recipient;
    let fingerprints: Vec<String> = keys.iter().map(crate::write::cert::fingerprint_of).collect();
    let names = |recipient: &Recipient, fingerprint: &str| match recipient {
        Recipient::KeyId { id } => fingerprint.len() == 40 && fingerprint.ends_with(id.as_str()),
        Recipient::Fingerprint { fingerprint: named } => named == fingerprint,
        Recipient::Hidden => false,
    };
    !found.passphrase
        && found.keys.iter().all(|r| fingerprints.iter().any(|f| names(r, f)))
        && fingerprints.iter().all(|f| found.keys.iter().any(|r| names(r, f)))
}

/// The waiting changes paw will not push, oldest first: each whose
/// ciphertext is not encrypted for exactly the recipients its `.gpg-id`
/// names now, as accepted in the keyring of `state_dir` (see [`HeldBack`]).
/// Empty if the store's recipients were never recorded (no setup), as
/// nothing paw saved was then pinned to them.
///
/// What to do with one: its text has to be encrypted again for the new
/// recipients, which needs the key. Accept the new recipients, then open
/// the entry and save it again (or take the change back with [`discard`]
/// and make it again); the old waiting change has to be discarded either
/// way, since pushing it would put the old ciphertext in the history.
pub fn held_back(dir: &Path, state_dir: &Path) -> Result<Vec<HeldBack>, SyncError> {
    let failed = |e: crate::write::WriteError| SyncError::Failed { message: e.to_string() };
    let _reading = StoreLock::for_repo(dir).shared()?;
    let keyring = crate::write::Keyring::open(state_dir).map_err(failed)?;
    let repo = super::guard::open(dir)?;
    let local = local(&repo)?;
    let now = crate::write::now_secs();
    let mut held = Vec::new();
    for commit in &local.commits {
        let (path, blob) = match &commit.change {
            Change::Write { path, blob } | Change::Rename { to: path, blob, .. } => (path, blob.0),
            Change::Delete { .. } => continue,
        };
        if !path.ends_with(".gpg") {
            continue;
        }
        let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
        let bytes = repo.find_blob(blob).map_err(git_err)?.content().to_vec();
        let found = crate::recipients::entry_recipients(&bytes).ok();
        let encrypted_for: Vec<String> = found
            .as_ref()
            .map(|f| f.keys.iter().map(crate::recipients::Recipient::describe).collect())
            .unwrap_or_default();
        let hold = |gpg_id: String, recipients: Vec<String>, accepted: bool| HeldBack {
            id: commit.commit.id().to_string(),
            entry: commit.entry(),
            gpg_id,
            encrypted_for: encrypted_for.clone(),
            recipients,
            accepted,
        };
        let recipients = match keyring.folder_recipients(dir, folder, now) {
            Ok(recipients) => recipients,
            Err(_) => {
                held.push(hold(String::new(), Vec::new(), false));
                continue;
            }
        };
        // `confirmed` is one fact about the whole store, not this folder:
        // setup has not recorded who can open it, so there is nothing to
        // compare against, and the write path refuses every save until it
        // has. Anything already found unreadable stays held.
        if !recipients.confirmed {
            return Ok(held);
        }
        let accepted = !recipients.changed;
        let exact = accepted
            && found.as_ref().is_some_and(|found| {
                keyring.resolve(dir, folder, now).is_ok_and(|keys| encrypted_for_exactly(found, &keys))
            });
        if !exact {
            let lines = recipients.lines.into_iter().map(|line| line.line).collect();
            held.push(hold(recipients.gpg_id, lines, accepted));
        }
    }
    Ok(held)
}

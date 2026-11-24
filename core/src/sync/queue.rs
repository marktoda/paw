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

/// What [`reencrypt_held_back`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reencrypted {
    /// Waiting changes now encrypted for exactly their entry's recipients:
    /// no longer held back.
    pub reencrypted: u32,
    /// Still held back: their folder's new recipients are not accepted on
    /// this device yet, so paw cannot encrypt to them.
    pub not_accepted: u32,
}

/// Encrypts every held-back change whose new recipients the person has
/// accepted again, for exactly those recipients, in place in the queue:
/// the same changes in the same order, with the same messages and times,
/// so the old ciphertext never reaches the remote. Needs each entry's
/// text, which `open` gives for the held-back change and its waiting
/// ciphertext (with the key, or a Face ID copy): never call it in the
/// background.
///
/// The store lock is never held while `open` runs (a card decrypt waits
/// for a touch). The ciphertext is read under the shared lock; the queue
/// is rewritten under the exclusive lock, and only if it is still exactly
/// what was read: a save or a send in between ends this with an error and
/// changes nothing. One branch move: either every one is re-encrypted, or
/// none is. Nothing is sent.
pub fn reencrypt_held_back(
    dir: &Path,
    state_dir: &Path,
    open: &mut dyn FnMut(&HeldBack, &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, SyncError>,
) -> Result<Reencrypted, SyncError> {
    let failed = |e: crate::write::WriteError| SyncError::Failed { message: e.to_string() };
    let held = held_back(dir, state_dir)?;
    let not_accepted = held.iter().filter(|h| !h.accepted).count() as u32;
    let ready: Vec<HeldBack> = held.into_iter().filter(|h| h.accepted).collect();
    if ready.is_empty() {
        return Ok(Reencrypted { reencrypted: 0, not_accepted });
    }
    let changed = || SyncError::Failed {
        message: "the waiting changes changed while paw was opening them; try again".into(),
    };

    // 1. The waiting ciphertext, read under the shared lock.
    let (head, waiting) = {
        let _reading = StoreLock::for_repo(dir).shared()?;
        let repo = super::guard::open(dir)?;
        let local = local(&repo)?;
        let mut waiting = Vec::new();
        for item in &ready {
            let commit = local.commits.iter().find(|c| c.commit.id().to_string() == item.id).ok_or_else(changed)?;
            let (path, blob) = match &commit.change {
                Change::Write { path, blob } | Change::Rename { to: path, blob, .. } => (path.clone(), blob.0),
                Change::Delete { .. } => return Err(changed()),
            };
            let bytes = repo.find_blob(blob).map_err(git_err)?.content().to_vec();
            waiting.push((item, path, blob, bytes));
        }
        (local.head, waiting)
    };

    // 2. Opened with no lock held.
    let mut opened = Vec::with_capacity(waiting.len());
    for (item, path, blob, bytes) in waiting {
        let text = open(item, &bytes)?;
        opened.push((path, blob, text));
    }

    // 3. Encrypted again and the queue rebuilt, under the exclusive lock.
    let store = StoreLock::for_repo(dir);
    let held = store.exclusive()?;
    let repo = super::guard::open(dir)?;
    let local = local(&repo)?;
    if local.head != head {
        return Err(changed());
    }
    ensure_clean(&repo)?;
    let keyring = crate::write::Keyring::open(state_dir).map_err(failed)?;
    let now = crate::write::now_secs();
    let mut swaps: std::collections::HashMap<Oid, Oid> = std::collections::HashMap::new();
    for (path, old, text) in &opened {
        let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
        let keys = keyring.resolve(dir, folder, now).map_err(failed)?;
        let ciphertext = crate::write::encrypt(&keys, text.as_slice()).map_err(failed)?;
        swaps.insert(*old, repo.blob(&ciphertext).map_err(git_err)?);
    }
    drop(opened);
    let Some(first) = local.commits.iter().position(|c| {
        matches!(&c.change, Change::Write { blob, .. } | Change::Rename { blob, .. } if swaps.contains_key(&blob.0))
    }) else {
        return Err(changed());
    };
    check_not_being_sent(&held, &store, &repo, &local.branch, local.commits[first].commit.id())?;
    // Every path a waiting change writes: where an old ciphertext can be.
    let paths: Vec<&str> = local.commits[first..]
        .iter()
        .filter_map(|c| match &c.change {
            Change::Write { path, .. } | Change::Rename { to: path, .. } => Some(path.as_str()),
            Change::Delete { .. } => None,
        })
        .collect();
    let mut tip = local.commits[first].commit.parent_id(0).map_err(git_err)?;
    for commit in &local.commits[first..] {
        let tree = commit.commit.tree().map_err(git_err)?;
        let mut index = git2::Index::new().map_err(git_err)?;
        index.read_tree(&tree).map_err(git_err)?;
        for path in &paths {
            if let Ok(entry) = tree.get_path(Path::new(path))
                && let Some(new) = swaps.get(&entry.id())
            {
                index.add(&super::history::index_entry(path, *new, entry.filemode() as u32)).map_err(git_err)?;
            }
        }
        let tree = repo.find_tree(index.write_tree_to(&repo).map_err(git_err)?).map_err(git_err)?;
        let parent = repo.find_commit(tip).map_err(git_err)?;
        let committer = commit.commit.committer();
        let committer =
            git2::Signature::now(committer.name().unwrap_or("paw"), committer.email().unwrap_or("")).map_err(git_err)?;
        let message = String::from_utf8_lossy(commit.commit.message_bytes()).into_owned();
        tip = repo.commit(None, &commit.commit.author(), &committer, &message, &tree, &[&parent]).map_err(git_err)?;
    }
    move_branch(&held, &repo, &local.branch, local.head, tip, None, "paw: re-encrypt waiting changes")?;
    Ok(Reencrypted { reencrypted: swaps.len() as u32, not_accepted })
}

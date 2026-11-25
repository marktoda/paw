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
/// recipients, which needs the key. Accept the new recipients, then
/// [`reencrypt_each`] (or take the change back with [`discard`]).
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

/// Gives the text of a held-back change from its waiting ciphertext (with
/// the key, or a Face ID copy), for [`reencrypt_held_back`].
pub type OpenWaiting<'a> = dyn FnMut(&HeldBack, &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, SyncError> + 'a;

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
    open: &mut OpenWaiting<'_>,
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
    // Keyed by path and blob: a rename that kept the blob puts the same
    // ciphertext at another path, whose folder may have other recipients,
    // and each path gets the ciphertext its own folder needs.
    let mut swaps: std::collections::HashMap<(String, Oid), Oid> = std::collections::HashMap::new();
    let reencrypted = opened.len() as u32;
    for (path, old, text) in &opened {
        let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
        let keys = keyring.resolve(dir, folder, now).map_err(failed)?;
        let ciphertext = crate::write::encrypt(&keys, text.as_slice()).map_err(failed)?;
        swaps.insert((path.clone(), *old), repo.blob(&ciphertext).map_err(git_err)?);
    }
    drop(opened);
    rewrite_with(&held, &store, &repo, &local, &swaps)?.ok_or_else(changed)?;
    Ok(Reencrypted { reencrypted, not_accepted })
}

/// Rebuilds the queue from the first change that holds one of `swaps`'
/// (path, old blob) pairs, with the new blob there and wherever a later
/// change still has that old blob at that path, and moves the branch once.
/// `None` if no change holds any of them. Called under the exclusive lock.
fn rewrite_with(
    held: &Exclusive,
    store: &StoreLock,
    repo: &Repository,
    local: &Local<'_>,
    swaps: &std::collections::HashMap<(String, Oid), Oid>,
) -> Result<Option<()>, SyncError> {
    let Some(first) = local.commits.iter().position(|c| match &c.change {
        Change::Write { path, blob } | Change::Rename { to: path, blob, .. } => {
            swaps.contains_key(&(path.clone(), blob.0))
        }
        Change::Delete { .. } => false,
    }) else {
        return Ok(None);
    };
    check_not_being_sent(held, store, repo, &local.branch, local.commits[first].commit.id())?;
    let mut tip = local.commits[first].commit.parent_id(0).map_err(git_err)?;
    for commit in &local.commits[first..] {
        let tree = commit.commit.tree().map_err(git_err)?;
        let mut index = git2::Index::new().map_err(git_err)?;
        index.read_tree(&tree).map_err(git_err)?;
        for ((path, old), new) in swaps {
            if let Ok(entry) = tree.get_path(Path::new(path))
                && entry.id() == *old
            {
                index.add(&super::history::index_entry(path, *new, entry.filemode() as u32)).map_err(git_err)?;
            }
        }
        let tree = repo.find_tree(index.write_tree_to(repo).map_err(git_err)?).map_err(git_err)?;
        let parent = repo.find_commit(tip).map_err(git_err)?;
        let committer = commit.commit.committer();
        let committer =
            git2::Signature::now(committer.name().unwrap_or("paw"), committer.email().unwrap_or("")).map_err(git_err)?;
        let message = String::from_utf8_lossy(commit.commit.message_bytes()).into_owned();
        tip = repo.commit(None, &commit.commit.author(), &committer, &message, &tree, &[&parent]).map_err(git_err)?;
    }
    move_branch(held, repo, &local.branch, local.head, tip, None, "paw: re-encrypt waiting changes")?;
    Ok(Some(()))
}

/// Why [`reencrypt_each`] left a held-back change as it was. It stays held
/// back; nothing of it is sent.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum LeftReason {
    /// Its folder's new list is not accepted on this device yet.
    NotAccepted,
    /// paw has no public key (or only an expired one) for these lines of
    /// its folder's new list.
    MissingKeys { missing: Vec<String> },
    /// paw can't read its folder's list of keys.
    NoList,
    /// The waiting change isn't encrypted for the key that was plugged in.
    NotForThisKey,
    /// The waiting change didn't open (damaged, or not text paw can read).
    Unreadable,
}

/// A held-back change [`reencrypt_each`] left, and why.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LeftHeldBack {
    /// As in [`HeldBack::id`] when it was looked at.
    pub id: String,
    pub entry: String,
    pub reason: LeftReason,
}

/// What can be re-encrypted now, without the key.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ReencryptCheck {
    /// Held-back changes paw can encrypt for their folder's new list, once
    /// the key has opened them: one touch each.
    pub ready: u32,
    /// The others, and why not.
    pub left: Vec<LeftHeldBack>,
}

/// What [`reencrypt_each`] did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ReencryptReport {
    /// Held-back changes now encrypted for exactly their folder's list:
    /// ordinary waiting changes again.
    pub reencrypted: u32,
    /// Still held back, and why.
    pub left: Vec<LeftHeldBack>,
}

/// Why [`OpenEach`] did not give a change's text.
#[derive(Debug)]
pub enum OpenFailure {
    /// This one can't be opened; leave it and go on with the next.
    Skip(LeftReason),
    /// Stop here (cancel, the key went away, no touch). What is already
    /// re-encrypted stays so.
    Stop(SyncError),
}

/// Gives the text of a held-back change from its waiting ciphertext, for
/// [`reencrypt_each`].
pub type OpenEach<'a> = dyn FnMut(&HeldBack, &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, OpenFailure> + 'a;

/// A held-back change, looked at for re-encrypting.
enum Look {
    Ready { path: String, blob: Oid },
    Left(LeftReason),
}

fn look(
    item: &HeldBack,
    local: &Local<'_>,
    keyring: &crate::write::Keyring,
    dir: &Path,
    now: u32,
) -> Option<Look> {
    let commit = local.commits.iter().find(|c| c.commit.id().to_string() == item.id)?;
    let (path, blob) = match &commit.change {
        Change::Write { path, blob } | Change::Rename { to: path, blob, .. } => (path.clone(), blob.0),
        Change::Delete { .. } => return None,
    };
    if item.gpg_id.is_empty() {
        return Some(Look::Left(LeftReason::NoList));
    }
    if !item.accepted {
        return Some(Look::Left(LeftReason::NotAccepted));
    }
    let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
    Some(match keyring.resolve(dir, folder, now) {
        Ok(_) => Look::Ready { path, blob },
        Err(crate::write::WriteError::MissingRecipients { missing, .. }) => {
            Look::Left(LeftReason::MissingKeys { missing })
        }
        Err(crate::write::WriteError::RecipientsChanged { .. }) => Look::Left(LeftReason::NotAccepted),
        Err(_) => Look::Left(LeftReason::NoList),
    })
}

/// The held-back changes as [`reencrypt_each`] sees them: how many are
/// ready, and why each other one is not (`skipped` holds those it already
/// tried, by path and waiting blob).
fn looked(
    dir: &Path,
    state_dir: &Path,
    skipped: &std::collections::HashMap<(String, Oid), LeftReason>,
) -> Result<ReencryptCheck, SyncError> {
    let failed = |e: crate::write::WriteError| SyncError::Failed { message: e.to_string() };
    let held = held_back(dir, state_dir)?;
    let _reading = StoreLock::for_repo(dir).shared()?;
    let keyring = crate::write::Keyring::open(state_dir).map_err(failed)?;
    let repo = super::guard::open(dir)?;
    let local = local(&repo)?;
    let now = crate::write::now_secs();
    let mut check = ReencryptCheck { ready: 0, left: Vec::new() };
    for item in &held {
        let reason = match look(item, &local, &keyring, dir, now) {
            Some(Look::Ready { path, blob }) => match skipped.get(&(path, blob)) {
                Some(reason) => reason.clone(),
                None => {
                    check.ready += 1;
                    continue;
                }
            },
            Some(Look::Left(reason)) => reason,
            None => continue,
        };
        check.left.push(LeftHeldBack { id: item.id.clone(), entry: item.entry.clone(), reason });
    }
    Ok(check)
}

/// How many held-back changes [`reencrypt_each`] could re-encrypt now, and
/// why the others can't be. No key, no network.
pub fn reencrypt_check(dir: &Path, state_dir: &Path) -> Result<ReencryptCheck, SyncError> {
    looked(dir, state_dir, &std::collections::HashMap::new())
}

/// Encrypts the held-back changes again for their folder's new list, one
/// at a time, oldest first: `open` gives each one's text (a touch of the
/// key), and the queue is rewritten for it at once, keeping the same
/// changes in the same order with the same messages and times. So a stop
/// part-way (`OpenFailure::Stop`, an error) keeps every one already done,
/// and running it again starts with the next. A change that can't be done
/// is left held back and said in `left`; nothing is ever sent, and the old
/// ciphertext never leaves the queue except by being replaced.
///
/// `progress` hears (done, total) before each one is opened and after it
/// is written. Two changes with the same waiting ciphertext are opened
/// once. The store lock is never held while `open` runs; a save in between
/// only makes it look again.
pub fn reencrypt_each(
    dir: &Path,
    state_dir: &Path,
    open: &mut OpenEach<'_>,
    progress: &mut dyn FnMut(u32, u32),
) -> Result<ReencryptReport, SyncError> {
    use std::collections::HashMap;
    let failed = |e: crate::write::WriteError| SyncError::Failed { message: e.to_string() };
    let mut done = 0;
    let mut total = reencrypt_check(dir, state_dir)?.ready;
    // Tried and left, by (path, waiting blob): ids change as the queue is
    // rewritten, these do not.
    let mut skipped: HashMap<(String, Oid), LeftReason> = HashMap::new();
    let mut texts: HashMap<Oid, zeroize::Zeroizing<Vec<u8>>> = HashMap::new();
    // Written this run, by (path, new blob): one that still looks held back
    // is never opened again.
    let mut written: std::collections::HashSet<(String, Oid)> = std::collections::HashSet::new();
    let mut moved = 0;
    loop {
        // 1. The next one to do, read under the shared lock.
        let next = {
            let held = held_back(dir, state_dir)?;
            let _reading = StoreLock::for_repo(dir).shared()?;
            let keyring = crate::write::Keyring::open(state_dir).map_err(failed)?;
            let repo = super::guard::open(dir)?;
            let local = local(&repo)?;
            let now = crate::write::now_secs();
            let mut next = None;
            for item in held {
                if let Some(Look::Ready { path, blob }) = look(&item, &local, &keyring, dir, now)
                    && !skipped.contains_key(&(path.clone(), blob))
                {
                    let bytes = repo.find_blob(blob).map_err(git_err)?.content().to_vec();
                    next = Some((item, path, blob, bytes, local.head));
                    break;
                }
            }
            next
        };
        let Some((item, path, blob, bytes, head)) = next else { break };
        if written.contains(&(path.clone(), blob)) {
            return Err(SyncError::Failed {
                message: "a change paw re-encrypted is still held back; nothing was sent".into(),
            });
        }
        total = total.max(done + 1);
        progress(done, total);

        // 2. Opened with no lock held, unless the same ciphertext already was.
        if let std::collections::hash_map::Entry::Vacant(slot) = texts.entry(blob) {
            match open(&item, &bytes) {
                Ok(text) => {
                    slot.insert(text);
                }
                Err(OpenFailure::Skip(reason)) => {
                    skipped.insert((path, blob), reason);
                    continue;
                }
                Err(OpenFailure::Stop(error)) => return Err(error),
            }
        }

        // 3. Encrypted for the folder's list and written, under the
        // exclusive lock, if the queue is still what was read.
        let store = StoreLock::for_repo(dir);
        let held = store.exclusive()?;
        let repo = super::guard::open(dir)?;
        let local = local(&repo)?;
        if local.head != head {
            // A save or a discard in between: look again (the text is kept).
            moved += 1;
            if moved > 3 {
                return Err(SyncError::Failed {
                    message: "the waiting changes kept changing while paw was re-encrypting them; try again".into(),
                });
            }
            continue;
        }
        ensure_clean(&repo)?;
        let keyring = crate::write::Keyring::open(state_dir).map_err(failed)?;
        let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
        let keys = keyring.resolve(dir, folder, crate::write::now_secs()).map_err(failed)?;
        let Some(text) = texts.get(&blob) else {
            return Err(SyncError::Failed { message: "internal: a held-back change's text went missing".into() });
        };
        let ciphertext = crate::write::encrypt(&keys, text.as_slice()).map_err(failed)?;
        let new_blob = repo.blob(&ciphertext).map_err(git_err)?;
        let mut swaps = HashMap::new();
        swaps.insert((path.clone(), blob), new_blob);
        if rewrite_with(&held, &store, &repo, &local, &swaps)?.is_none() {
            return Err(SyncError::Failed { message: "internal: a held-back change was not in the queue".into() });
        }
        written.insert((path, new_blob));
        moved = 0;
        done += 1;
        progress(done, total);
    }
    Ok(ReencryptReport { reencrypted: done, left: looked(dir, state_dir, &skipped)?.left })
}

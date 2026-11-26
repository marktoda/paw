//! Waiting changes encrypted for recipients their entry no longer has,
//! and encrypting them again: a send pushes none of the queue while any is
//! held back ([`holds_back`]).

use std::path::Path;

use git2::{Oid, Repository};

use super::{Keyring, WriteError, encrypt, now_secs};
use crate::key::KeyError;
use crate::lock::{Exclusive, StoreLock};
use crate::sync::history::{ensure_clean, index_entry, move_branch};
use crate::sync::queue::{Waiting, waiting};
use crate::sync::{SyncError, check_not_being_sent, git_err, guard};

impl From<WriteError> for SyncError {
    fn from(error: WriteError) -> Self {
        match error {
            WriteError::StoreBusy => Self::StoreBusy,
            other => Self::Failed { message: other.to_string() },
        }
    }
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
    /// The `.gpg-id` that governs the entry now, e.g. `work/.gpg-id`;
    /// `None` if it can't be read.
    pub gpg_id: Option<String>,
    /// Who the waiting ciphertext is encrypted for, as recipients are
    /// described elsewhere ("key ID 0123...").
    pub encrypted_for: Vec<String>,
    /// The key lines of that `.gpg-id` now.
    pub recipients: Vec<String>,
    pub list: HeldBackList,
}

/// Where a held-back change's folder's list of keys stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HeldBackList {
    /// The person has accepted it on this device.
    Accepted,
    /// Not accepted yet. Until it is, paw cannot encrypt to it either.
    NotAccepted,
    /// paw can't read it.
    Unreadable,
}

/// Whether `found` names exactly the keys of `keys`, no more, no fewer,
/// and nothing hidden or passphrase-only.
fn encrypted_for_exactly(found: &crate::recipients::EntryRecipients, keys: &[pgp::packet::PublicKey]) -> bool {
    let fingerprints: Vec<String> = keys.iter().map(crate::recipients::fingerprint_of).collect();
    !found.passphrase
        && found.keys.iter().all(|r| fingerprints.iter().any(|f| r.names(f)))
        && fingerprints.iter().all(|f| found.keys.iter().any(|r| r.names(f)))
}

/// The waiting changes paw will not push, oldest first: each whose
/// ciphertext is not encrypted for exactly the recipients its `.gpg-id`
/// names now, as accepted in the keyring of `state_dir` (see [`HeldBack`]).
/// Empty if the store's recipients were never recorded (no setup), as
/// nothing paw saved was then pinned to them.
///
/// What to do with one: its text has to be encrypted again for the new
/// recipients, which needs the key. Accept the new recipients, then
/// [`reencrypt_each`] (or take the change back with [`crate::sync::queue::discard`]).
pub fn held_back(dir: &Path, state_dir: &Path) -> Result<Vec<HeldBack>, SyncError> {
    let _reading = StoreLock::for_repo(dir).shared()?;
    let keyring = Keyring::open(state_dir)?;
    // Setup has not recorded who can open the store, so there is nothing to
    // compare against, and the write path refuses every save until it has.
    if !keyring.confirmed() {
        return Ok(Vec::new());
    }
    let repo = guard::open(dir)?;
    let local = waiting(&repo)?;
    let now = now_secs();
    let mut held = Vec::new();
    for commit in &local.commits {
        let Some((path, blob)) = commit.change.ciphertext() else { continue };
        if !crate::path::is_entry_file(path) {
            continue;
        }
        let folder = crate::path::folder_of(path);
        let bytes = repo.find_blob(blob).map_err(git_err)?.content().to_vec();
        let found = crate::recipients::entry_recipients(&bytes).ok();
        let encrypted_for: Vec<String> = found
            .as_ref()
            .map(|f| f.keys.iter().map(crate::recipients::Recipient::describe).collect())
            .unwrap_or_default();
        let hold = |gpg_id: Option<String>, recipients: Vec<String>, list: HeldBackList| HeldBack {
            id: commit.commit.id().to_string(),
            entry: commit.entry(),
            gpg_id,
            encrypted_for: encrypted_for.clone(),
            recipients,
            list,
        };
        let recipients = match keyring.folder_recipients(dir, folder, now) {
            Ok(recipients) => recipients,
            Err(_) => {
                held.push(hold(None, Vec::new(), HeldBackList::Unreadable));
                continue;
            }
        };
        let accepted = !recipients.changed;
        let exact = accepted
            && found.as_ref().is_some_and(|found| {
                keyring.resolve(dir, folder, now).is_ok_and(|keys| encrypted_for_exactly(found, &keys))
            });
        if !exact {
            let lines = recipients.lines.into_iter().map(|line| line.line).collect();
            let list = if accepted { HeldBackList::Accepted } else { HeldBackList::NotAccepted };
            held.push(hold(Some(recipients.gpg_id), lines, list));
        }
    }
    Ok(held)
}

/// Rebuilds the queue from the first change that holds one of `swaps`'
/// (path, old blob) pairs, with the new blob there and wherever a later
/// change still has that old blob at that path, and moves the branch once.
/// `None` if no change holds any of them. Called under the exclusive lock.
fn rewrite_with(
    held: &Exclusive,
    store: &StoreLock,
    repo: &Repository,
    local: &Waiting<'_>,
    swaps: &std::collections::HashMap<(String, Oid), Oid>,
) -> Result<Option<()>, SyncError> {
    let Some(first) = local.commits.iter().position(|c| {
        c.change.ciphertext().is_some_and(|(path, blob)| swaps.contains_key(&(path.to_owned(), blob)))
    }) else {
        return Ok(None);
    };
    check_not_being_sent(held, store, repo, &local.short, local.commits[first].commit.id())?;
    let mut tip = local.commits[first].commit.parent_id(0).map_err(git_err)?;
    for commit in &local.commits[first..] {
        let tree = commit.commit.tree().map_err(git_err)?;
        let mut index = git2::Index::new().map_err(git_err)?;
        index.read_tree(&tree).map_err(git_err)?;
        for ((path, old), new) in swaps {
            if let Ok(entry) = tree.get_path(Path::new(path))
                && entry.id() == *old
            {
                index.add(&index_entry(path, *new, entry.filemode() as u32)).map_err(git_err)?;
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
pub enum OpenFailure<S = SyncError> {
    /// This one can't be opened; leave it and go on with the next.
    Skip(LeftReason),
    /// Stop here (cancel, the key went away, no touch), and end
    /// [`reencrypt_each`] with `S`. What is already re-encrypted stays so.
    Stop(S),
}

/// What the key's failure to open one change means: that change is left
/// (not encrypted for this key, or damaged), or the whole run stops.
impl<S: From<KeyError>> From<KeyError> for OpenFailure<S> {
    fn from(error: KeyError) -> Self {
        match error {
            KeyError::NotForThisKey { .. } => Self::Skip(LeftReason::NotForThisKey),
            KeyError::EntryUnreadable { .. } | KeyError::DecryptFailed { .. } => Self::Skip(LeftReason::Unreadable),
            other => Self::Stop(other.into()),
        }
    }
}

/// Gives the text of a held-back change from its waiting ciphertext, for
/// [`reencrypt_each`].
pub type OpenEach<'a, S = SyncError> =
    dyn FnMut(&HeldBack, &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, OpenFailure<S>> + 'a;

/// A run that keeps finding the queue changed under it gives up after
/// this many looks in a row.
const MOST_LOOKS_AGAIN: u32 = 3;

/// How [`reencrypt_each`] stopped early.
enum Ended<S> {
    Failed(SyncError),
    Stopped(S),
}

impl<S, E: Into<SyncError>> From<E> for Ended<S> {
    fn from(error: E) -> Self {
        Self::Failed(error.into())
    }
}

/// A held-back change, looked at for re-encrypting.
enum Look {
    Ready { path: String, blob: Oid },
    Left(LeftReason),
}

fn look(
    item: &HeldBack,
    local: &Waiting<'_>,
    keyring: &Keyring,
    dir: &Path,
    now: u32,
) -> Option<Look> {
    let commit = local.commits.iter().find(|c| c.commit.id().to_string() == item.id)?;
    let (path, blob) = commit.change.ciphertext()?;
    let path = path.to_owned();
    match item.list {
        HeldBackList::Unreadable => return Some(Look::Left(LeftReason::NoList)),
        HeldBackList::NotAccepted => return Some(Look::Left(LeftReason::NotAccepted)),
        HeldBackList::Accepted => {}
    }
    let folder = crate::path::folder_of(&path);
    Some(match keyring.resolve(dir, folder, now) {
        Ok(_) => Look::Ready { path, blob },
        Err(WriteError::MissingRecipients { missing, .. }) => {
            Look::Left(LeftReason::MissingKeys { missing })
        }
        Err(WriteError::RecipientsChanged { .. }) => Look::Left(LeftReason::NotAccepted),
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
    let held = held_back(dir, state_dir)?;
    let _reading = StoreLock::for_repo(dir).shared()?;
    let keyring = Keyring::open(state_dir)?;
    let repo = guard::open(dir)?;
    let local = waiting(&repo)?;
    let now = now_secs();
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
pub fn reencrypt_each<S: From<SyncError>>(
    dir: &Path,
    state_dir: &Path,
    open: &mut OpenEach<'_, S>,
    progress: &mut dyn FnMut(u32, u32),
) -> Result<ReencryptReport, S> {
    match reencrypt_each_until(dir, state_dir, open, progress) {
        Ok(report) => Ok(report),
        Err(Ended::Failed(error)) => Err(error.into()),
        Err(Ended::Stopped(stop)) => Err(stop),
    }
}

fn reencrypt_each_until<S>(
    dir: &Path,
    state_dir: &Path,
    open: &mut OpenEach<'_, S>,
    progress: &mut dyn FnMut(u32, u32),
) -> Result<ReencryptReport, Ended<S>> {
    use std::collections::HashMap;
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
            let keyring = Keyring::open(state_dir)?;
            let repo = guard::open(dir)?;
            let local = waiting(&repo)?;
            let now = now_secs();
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
            }
            .into());
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
                Err(OpenFailure::Stop(stop)) => return Err(Ended::Stopped(stop)),
            }
        }

        // 3. Encrypted for the folder's list and written, under the
        // exclusive lock, if the queue is still what was read.
        let store = StoreLock::for_repo(dir);
        let held = store.exclusive()?;
        let repo = guard::open(dir)?;
        let local = waiting(&repo)?;
        if local.head != head {
            // A save or a discard in between: look again (the text is kept).
            moved += 1;
            if moved > MOST_LOOKS_AGAIN {
                return Err(SyncError::QueueKeptChanging.into());
            }
            continue;
        }
        ensure_clean(&repo)?;
        let keyring = Keyring::open(state_dir)?;
        let folder = crate::path::folder_of(&path);
        let keys = keyring.resolve(dir, folder, now_secs())?;
        let Some(text) = texts.get(&blob) else {
            return Err(SyncError::Failed { message: "internal: a held-back change's text went missing".into() }.into());
        };
        let ciphertext = encrypt(&keys, text.as_slice())?;
        let new_blob = repo.blob(&ciphertext).map_err(git_err)?;
        let mut swaps = HashMap::new();
        swaps.insert((path.clone(), blob), new_blob);
        if rewrite_with(&held, &store, &repo, &local, &swaps)?.is_none() {
            return Err(SyncError::Failed { message: "internal: a held-back change was not in the queue".into() }.into());
        }
        written.insert((path, new_blob));
        moved = 0;
        done += 1;
        progress(done, total);
    }
    Ok(ReencryptReport { reencrypted: done, left: looked(dir, state_dir, &skipped)?.left })
}

/// Whether any waiting change is held back: a send then pushes nothing.
pub(crate) fn holds_back(dir: &Path, state_dir: &Path) -> Result<bool, SyncError> {
    Ok(!held_back(dir, state_dir)?.is_empty())
}

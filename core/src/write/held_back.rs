//! Waiting changes encrypted for recipients their entry no longer has,
//! and encrypting them again: a send pushes none of the queue while any is
//! held back ([`holds_back`]).

use std::path::Path;

use git2::Oid;

use super::{Keyring, WriteError, encrypt};
use crate::fault::Fault;
use crate::key::KeyError;
use crate::lock::StoreLock;
use crate::sync::SyncError;
use crate::sync::queue::{Rewritten, WaitingFile, rewrite_ciphertexts, waiting_files};

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
    pub encrypted_for: Vec<crate::recipients::Recipient>,
    /// The key lines of that `.gpg-id` now.
    pub recipients: Vec<String>,
    pub list: HeldBackList,
}

/// Where a held-back change's folder's list of keys stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HeldBackList {
    Accepted,
    /// Not accepted yet. Until it is, paw cannot encrypt to it either.
    NotAccepted,
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
    let (_, files) = waiting_files(dir)?;
    let mut held = Vec::new();
    for file in files {
        if !crate::path::is_entry_file(&file.path) {
            continue;
        }
        let folder = crate::path::folder_of(&file.path);
        let found = crate::recipients::entry_recipients(&file.ciphertext).ok();
        let encrypted_for = found.as_ref().map(|f| f.keys.clone()).unwrap_or_default();
        let hold = |gpg_id: Option<String>, recipients: Vec<String>, list: HeldBackList| HeldBack {
            id: file.id.clone(),
            entry: file.entry.clone(),
            gpg_id,
            encrypted_for: encrypted_for.clone(),
            recipients,
            list,
        };
        let recipients = match keyring.folder_recipients(dir, folder) {
            Ok(recipients) => recipients,
            Err(_) => {
                held.push(hold(None, Vec::new(), HeldBackList::Unreadable));
                continue;
            }
        };
        let accepted = !recipients.changed;
        let exact = accepted
            && found.as_ref().is_some_and(|found| {
                keyring.resolve(dir, folder).is_ok_and(|keys| encrypted_for_exactly(found, &keys))
            });
        if !exact {
            let lines = recipients.lines.into_iter().map(|line| line.line).collect();
            let list = if accepted { HeldBackList::Accepted } else { HeldBackList::NotAccepted };
            held.push(hold(Some(recipients.gpg_id), lines, list));
        }
    }
    Ok(held)
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
    pub left: Vec<LeftHeldBack>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ReencryptReport {
    /// Held-back changes now encrypted for exactly their folder's list:
    /// ordinary waiting changes again.
    pub reencrypted: u32,
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

enum Ended<S> {
    Failed(SyncError),
    Stopped(S),
}

impl<S, E: Into<SyncError>> From<E> for Ended<S> {
    fn from(error: E) -> Self {
        Self::Failed(error.into())
    }
}

enum Look {
    Ready { path: String, blob: Oid },
    Left(LeftReason),
}

/// Why a change in a folder whose keys can't be resolved is left.
fn left_reason(error: WriteError) -> LeftReason {
    match error {
        WriteError::MissingRecipients { missing, .. } => LeftReason::MissingKeys { missing },
        WriteError::RecipientsChanged { .. } => LeftReason::NotAccepted,
        _ => LeftReason::NoList,
    }
}

fn look(item: &HeldBack, files: &[WaitingFile], keyring: &Keyring, dir: &Path) -> Option<Look> {
    let file = files.iter().find(|file| file.id == item.id)?;
    match item.list {
        HeldBackList::Unreadable => return Some(Look::Left(LeftReason::NoList)),
        HeldBackList::NotAccepted => return Some(Look::Left(LeftReason::NotAccepted)),
        HeldBackList::Accepted => {}
    }
    let folder = crate::path::folder_of(&file.path);
    Some(match keyring.resolve(dir, folder) {
        Ok(_) => Look::Ready { path: file.path.clone(), blob: file.blob },
        Err(error) => Look::Left(left_reason(error)),
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
    let (_, files) = waiting_files(dir)?;
    let mut check = ReencryptCheck { ready: 0, left: Vec::new() };
    for item in &held {
        let reason = match look(item, &files, &keyring, dir) {
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
        let next = {
            let held = held_back(dir, state_dir)?;
            let _reading = StoreLock::for_repo(dir).shared()?;
            let keyring = Keyring::open(state_dir)?;
            let (head, files) = waiting_files(dir)?;
            held.into_iter().find_map(|item| match look(&item, &files, &keyring, dir) {
                Some(Look::Ready { path, blob }) if !skipped.contains_key(&(path.clone(), blob)) => {
                    let bytes = files.iter().find(|file| file.id == item.id).map(|file| file.ciphertext.clone())?;
                    Some((item, path, blob, bytes, head))
                }
                _ => None,
            })
        };
        let Some((item, path, blob, bytes, head)) = next else { break };
        if written.contains(&(path.clone(), blob)) {
            return Err(Fault::local("a change paw re-encrypted is still held back; nothing was sent").into());
        }
        total = total.max(done + 1);
        progress(done, total);

        // Opened with no lock held, unless the same ciphertext already was.
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

        let Some(text) = texts.get(&blob) else {
            return Err(Fault::local("internal: a held-back change's text went missing").into());
        };
        let mut left = None;
        let mut new_blob = None;
        let mut encrypt_again = |at: &str, old: Oid| -> Result<Option<Vec<u8>>, SyncError> {
            if (at, old) != (path.as_str(), blob) {
                return Ok(None);
            }
            let folder = crate::path::folder_of(&path);
            let keys = match Keyring::open(state_dir)?.resolve(dir, folder) {
                Ok(keys) => keys,
                Err(WriteError::Fault { fault }) => return Err(fault.into()),
                Err(other) => {
                    left = Some(left_reason(other));
                    return Ok(None);
                }
            };
            let ciphertext = encrypt(&keys, text.as_slice())?;
            new_blob = Some(Oid::hash_object(git2::ObjectType::Blob, &ciphertext).map_err(crate::sync::git_err)?);
            Ok(Some(ciphertext))
        };
        match rewrite_ciphertexts(dir, head, &mut encrypt_again)? {
            Rewritten::Moved => {
                // A save or a discard in between: look again (the text is kept).
                moved += 1;
                if moved > MOST_LOOKS_AGAIN {
                    return Err(SyncError::QueueKeptChanging.into());
                }
                continue;
            }
            Rewritten::Unchanged => {
                let Some(reason) = left else {
                    return Err(Fault::local("internal: a held-back change was not in the queue").into());
                };
                skipped.insert((path, blob), reason);
                continue;
            }
            Rewritten::Done => {}
        }
        written.extend(new_blob.map(|new_blob| (path, new_blob)));
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

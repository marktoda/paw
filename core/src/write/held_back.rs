//! Waiting changes encrypted for recipients their entry no longer has,
//! and encrypting them again: a send pushes none of the queue while any is
//! held back ([`holds_back`]).

use git2::Oid;

use super::{Keyring, WriteError, encrypt};
use crate::fault::Fault;
use crate::fingerprint::Fingerprint;
use crate::files::layout::StoreDirs;
use crate::key::KeyError;
use crate::lock::Shared;
use crate::sync::{OpenStore, SyncError};
use crate::sync::queue::{Rewritten, rewrite_ciphertexts, waiting_files};

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
    let fingerprints: Vec<Fingerprint> = keys.iter().map(Fingerprint::of).collect();
    !found.passphrase
        && found.keys.iter().all(|r| fingerprints.iter().any(|f| r.names(f)))
        && fingerprints.iter().all(|f| found.keys.iter().any(|r| r.names(f)))
}

/// The waiting changes paw will not push, oldest first: each whose
/// ciphertext is not encrypted for exactly the recipients its `.gpg-id`
/// names now, as accepted in the store's keyring (see [`HeldBack`]).
/// Empty if the store's recipients were never recorded (no setup), as
/// nothing paw saved was then pinned to them.
///
/// What to do with one: its text has to be encrypted again for the new
/// recipients, which needs the key. Accept the new recipients, then
/// [`reencrypt_each`] (or take the change back with [`crate::sync::queue::discard`]).
pub fn held_back(dirs: &StoreDirs) -> Result<Vec<HeldBack>, SyncError> {
    let reading = dirs.shared()?;
    held_back_in(&reading, &OpenStore::open(dirs)?)
}

pub(crate) fn held_back_in(reading: &Shared, store: &OpenStore) -> Result<Vec<HeldBack>, SyncError> {
    Ok(survey(reading, store)?.held.into_iter().map(|held| held.item).collect())
}

/// A held-back change, with what re-encrypting it needs.
struct Held {
    item: HeldBack,
    path: String,
    /// Its waiting ciphertext, and that blob's id.
    ciphertext: Vec<u8>,
    blob: Oid,
    look: Look,
}

/// The held-back changes, oldest first, and the branch tip they were read
/// at: everything [`held_back`], [`reencrypt_check`] and
/// [`reencrypt_each`] say about them, read once.
struct Survey {
    head: Oid,
    held: Vec<Held>,
}

fn survey(reading: &Shared, store: &OpenStore) -> Result<Survey, SyncError> {
    let keyring = Keyring::open(&store.dirs)?;
    // With nothing held back, no rewrite reads the tip.
    if !keyring.confirmed() {
        return Ok(Survey { head: Oid::ZERO_SHA1, held: Vec::new() });
    }
    let (head, files) = waiting_files(reading, store)?;
    let mut held = Vec::new();
    for file in files {
        if crate::path::entry_of_file(&file.path).is_none() {
            continue;
        }
        let folder = crate::path::folder_of(&file.path);
        let found = crate::recipients::entry_recipients(&file.ciphertext).ok();
        let (gpg_id, recipients, list, look) = match keyring.folder_recipients(reading, folder) {
            Err(_) => (None, Vec::new(), HeldBackList::Unreadable, Look::Left(LeftReason::NoList)),
            Ok(recipients) if recipients.changed => {
                let lines = recipients.lines.into_iter().map(|line| line.line).collect();
                (Some(recipients.gpg_id), lines, HeldBackList::NotAccepted, Look::Left(LeftReason::NotAccepted))
            }
            Ok(recipients) => {
                let keys = keyring.resolve(reading, folder);
                let exact = found.as_ref().is_some_and(|found| keys.as_ref().is_ok_and(|keys| encrypted_for_exactly(found, keys)));
                if exact {
                    continue;
                }
                let lines = recipients.lines.into_iter().map(|line| line.line).collect();
                let look = keys.map_or_else(|error| Look::Left(left_reason(error)), |_| Look::Ready);
                (Some(recipients.gpg_id), lines, HeldBackList::Accepted, look)
            }
        };
        let item = HeldBack {
            id: file.id,
            entry: file.entry,
            gpg_id,
            encrypted_for: found.map(|found| found.keys).unwrap_or_default(),
            recipients,
            list,
        };
        held.push(Held { item, path: file.path, ciphertext: file.ciphertext, blob: file.blob, look });
    }
    Ok(Survey { head, held })
}

/// Why [`reencrypt_each`] left a held-back change as it was. It stays held
/// back; nothing of it is sent.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum LeftReason {
    /// Its folder's new list is not accepted on this device yet.
    NotAccepted,
    /// paw has no public key for these lines of its folder's new list.
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
    Ready,
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

/// The held-back changes as [`reencrypt_each`] sees them: how many are
/// ready, and why each other one is not (`skipped` holds those it already
/// tried, by path and waiting blob).
fn looked(
    dirs: &StoreDirs,
    skipped: &std::collections::HashMap<(String, Oid), LeftReason>,
) -> Result<ReencryptCheck, SyncError> {
    let reading = dirs.shared()?;
    let mut check = ReencryptCheck { ready: 0, left: Vec::new() };
    for Held { item, path, blob, look, .. } in survey(&reading, &OpenStore::open(dirs)?)?.held {
        let reason = match look {
            Look::Ready => match skipped.get(&(path, blob)) {
                Some(reason) => reason.clone(),
                None => {
                    check.ready += 1;
                    continue;
                }
            },
            Look::Left(reason) => reason,
        };
        check.left.push(LeftHeldBack { id: item.id, entry: item.entry, reason });
    }
    Ok(check)
}

/// How many held-back changes [`reencrypt_each`] could re-encrypt now, and
/// why the others can't be. No key, no network.
pub fn reencrypt_check(dirs: &StoreDirs) -> Result<ReencryptCheck, SyncError> {
    looked(dirs, &std::collections::HashMap::new())
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
    dirs: &StoreDirs,
    open: &mut OpenEach<'_, S>,
    progress: &mut dyn FnMut(u32, u32),
) -> Result<ReencryptReport, S> {
    match reencrypt_each_until(dirs, open, progress) {
        Ok(report) => Ok(report),
        Err(Ended::Failed(error)) => Err(error.into()),
        Err(Ended::Stopped(stop)) => Err(stop),
    }
}

fn reencrypt_each_until<S>(
    dirs: &StoreDirs,
    open: &mut OpenEach<'_, S>,
    progress: &mut dyn FnMut(u32, u32),
) -> Result<ReencryptReport, Ended<S>> {
    use std::collections::HashMap;
    let mut done = 0;
    let mut total = reencrypt_check(dirs)?.ready;
    // Tried and left, by (path, waiting blob): ids change as the queue is
    // rewritten, these do not.
    let mut skipped: HashMap<(String, Oid), LeftReason> = HashMap::new();
    let mut texts: HashMap<Oid, zeroize::Zeroizing<Vec<u8>>> = HashMap::new();
    // Written this run, by (path, new blob): one that still looks held back
    // is never opened again.
    let mut written: std::collections::HashSet<(String, Oid)> = std::collections::HashSet::new();
    let mut moved = 0;
    loop {
        let Survey { head, held } = {
            let reading = dirs.shared()?;
            survey(&reading, &OpenStore::open(dirs)?)?
        };
        let next = held.into_iter().find(|held| matches!(held.look, Look::Ready) && !skipped.contains_key(&(held.path.clone(), held.blob)));
        let Some(Held { item, path, blob, ciphertext: bytes, .. }) = next else { break };
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
        let held = dirs.exclusive()?;
        let store = OpenStore::open(dirs)?;
        let mut left = None;
        let mut new_blob = None;
        let mut encrypt_again = |at: &str, old: Oid| -> Result<Option<Vec<u8>>, SyncError> {
            if (at, old) != (path.as_str(), blob) {
                return Ok(None);
            }
            let folder = crate::path::folder_of(&path);
            let keys = match Keyring::open(dirs)?.resolve(&held, folder) {
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
        let rewritten = rewrite_ciphertexts(&held, &store, head, &mut encrypt_again)?;
        drop(held);
        match rewritten {
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
    Ok(ReencryptReport { reencrypted: done, left: looked(dirs, &skipped)?.left })
}

/// Whether any waiting change is held back: a send then pushes nothing.
pub(crate) fn holds_back(reading: &Shared, store: &OpenStore) -> Result<bool, SyncError> {
    Ok(!survey(reading, store)?.held.is_empty())
}

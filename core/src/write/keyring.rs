//! The public keys paw encrypts to, pinned on this device.
//!
//! `.gpg-id` names recipients by fingerprint, key ID or e-mail address,
//! but encrypting needs their public keys, and the repository is not a
//! trusted place to get them from. So the keys live here, in the store's
//! state directory (not in the git repository). The only way in is the
//! card's own decryption key, pinned at setup.
//!
//! A `.gpg-id` line is satisfied by:
//! 1. a binding: an explicit record that this line means this pinned key.
//!    Setup makes one after the test decrypt, because the owner's
//!    `.gpg-id` usually names the primary key's fingerprint while the card
//!    exposes only its encryption subkey, and nothing on the phone can
//!    prove the two belong together except the person and that decrypt;
//! 2. otherwise, exactly one pinned key that the line names by its full
//!    fingerprint or long key ID.
//!
//! Short key IDs, names and e-mail addresses are never matched by
//! themselves: they need a binding. Resolving a folder is all or nothing:
//! if any line of its `.gpg-id` is not satisfied, saving there is refused
//! with every such line named. paw never encrypts to fewer keys than
//! `.gpg-id` names.
//!
//! paw always says when recipients change. Setup records every `.gpg-id`
//! in the store, with its contents ([`Keyring::accept_all_recipients_at_setup`]);
//! nothing is ever accepted implicitly after that. For each folder the
//! `.gpg-id` that governs it now is compared with the one that governed it
//! as accepted: another file (one added deeper, or one removed), or other
//! contents, refuses saving there with [`WriteError::RecipientsChanged`]
//! naming the old and new recipients, until the person accepts the folder
//! ([`Keyring::accept_recipients`]). The record is in the state directory,
//! which a pull never touches. Before setup has recorded anything, saving
//! is refused everywhere ([`WriteError::RecipientsNotConfirmed`]).
//!
//! The file is small text, written whole by rename. Nothing in it is
//! secret. A `key` line keeps the fields an earlier paw had for a key
//! taken from a certificate (primary key, expiry, fingerprints, e-mail
//! addresses), always `-` for the card's key. No phone was ever given such
//! a key; its `key` and `sub` lines are not read.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use pgp::packet::{Packet, PacketParser, PacketTrait, PublicKey};
use sha2::{Digest, Sha256};

use super::WriteError;
use crate::fault::Fault;
use crate::fingerprint::{Fingerprint, hex};
use crate::path::{folder_and_parents, folder_of, folder_of_gpg_id, gpg_id_in, governing};
use crate::store::gpg_id::{governing_gpg_id, gpg_id_text, key_digits, parse_gpg_id};
use crate::files::{MAX_FILE_BYTES, read_bounded, write_atomic};
use crate::lock::{Exclusive, Shared, StoreDirs};

const HEADER: &str = "# paw pinned recipients, version 1";

/// A public key paw may encrypt to: the card's decryption key.
#[derive(Clone)]
pub struct PinnedKey {
    /// The fingerprint of the key itself.
    pub fingerprint: Fingerprint,
    pub key: PublicKey,
}

impl std::fmt::Debug for PinnedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PinnedKey({})", self.fingerprint)
    }
}

/// What one `.gpg-id` line looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Spec {
    Fingerprint(Fingerprint),
    /// 16 hex digits.
    LongId(String),
    /// A short key ID, a name, an e-mail address or anything else: only a
    /// binding helps.
    Other,
}

impl Spec {
    /// Whether this line names the (sub)key with `fingerprint` by itself:
    /// by that fingerprint or by its key ID, as a recipient packet would.
    fn names(&self, fingerprint: &Fingerprint) -> bool {
        match self {
            Self::Fingerprint(wanted) => wanted == fingerprint,
            Self::LongId(id) => fingerprint.key_id() == id,
            Self::Other => false,
        }
    }
}

/// The 64-bit key ID a recipient packet would name for the key `line`
/// names by fingerprint or long key ID; `None` for any other line.
pub(crate) fn line_key_id(line: &str) -> Option<String> {
    match spec(line) {
        Spec::Fingerprint(fingerprint) => Some(fingerprint.key_id().to_owned()),
        Spec::LongId(id) => Some(id),
        Spec::Other => None,
    }
}

/// The fingerprint `line` names a key by, if it does.
pub(crate) fn line_fingerprint(line: &str) -> Option<Fingerprint> {
    match spec(line) {
        Spec::Fingerprint(fingerprint) => Some(fingerprint),
        Spec::LongId(_) | Spec::Other => None,
    }
}

fn spec(line: &str) -> Spec {
    match key_digits(line) {
        Some(digits) if digits.len() == 16 => Spec::LongId(digits),
        Some(digits) => Fingerprint::parse(&digits).map_or(Spec::Other, Spec::Fingerprint),
        None => Spec::Other,
    }
}

/// How one `.gpg-id` line is satisfied, for the screen that shows which
/// keys can open a folder.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum LineStatus {
    /// Encrypting uses the pinned key with this fingerprint.
    Ready { fingerprint: String, bound: bool },
    /// No pinned key matches.
    Missing,
}

impl LineStatus {
    /// The status of a line [`Keyring::line_key`] found `found` for.
    pub(crate) fn of(found: Option<(&PinnedKey, bool)>) -> Self {
        match found {
            Some((key, bound)) => Self::Ready { fingerprint: key.fingerprint.to_string(), bound },
            None => Self::Missing,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RecipientLine {
    pub line: String,
    pub status: LineStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FolderRecipients {
    /// The `.gpg-id` that decides them, e.g. `work/.gpg-id`.
    pub gpg_id: String,
    pub lines: Vec<RecipientLine>,
    /// Setup has recorded the store's recipients.
    pub confirmed: bool,
    /// The recipients differ from the ones accepted for this folder: another
    /// `.gpg-id` governs it now, or its contents changed.
    pub changed: bool,
    /// The `.gpg-id` that governed the folder as accepted, if any.
    pub accepted_gpg_id: Option<String>,
    /// Its key lines as accepted.
    pub accepted_lines: Vec<String>,
    /// Whether paw can save in the folder now: confirmed, unchanged, and
    /// every line ready.
    pub writable: bool,
}

/// A `.gpg-id` as the person accepted it.
#[derive(Clone)]
struct Accepted {
    path: String,
    sha: String,
    text: String,
}

/// Every `.gpg-id` committed in the store at `root` (its current branch),
/// outside hidden folders, as `(path, bytes)`.
fn all_gpg_ids(root: &Path) -> Result<Vec<(String, Vec<u8>)>, Fault> {
    let git = |e: git2::Error| Fault::local(format!("could not read the store: {}", e.message()));
    let repo = crate::sync::guard::open(root)?;
    let tree = repo.head().and_then(|h| h.peel_to_tree()).map_err(git)?;
    let mut found = Vec::new();
    tree.walk(git2::TreeWalkMode::PreOrder, |folder, entry| {
        let name = entry.name().unwrap_or_default();
        if entry.kind() == Some(git2::ObjectType::Tree) {
            // Hidden folders (`.git`-like, `.extensions`) are not the store's.
            return if crate::path::is_hidden(name) { git2::TreeWalkResult::Skip } else { git2::TreeWalkResult::Ok };
        }
        if name == crate::path::GPG_ID
            && entry.filemode() == i32::from(git2::FileMode::Blob)
            && let Ok(blob) = repo.find_blob(entry.id())
        {
            found.push((format!("{folder}{name}"), blob.content().to_vec()));
        }
        git2::TreeWalkResult::Ok
    })
    .map_err(git)?;
    Ok(found)
}

/// The paths of every `.gpg-id` committed in the store at `root`, outside
/// hidden folders, e.g. `.gpg-id` and `archive/.gpg-id`.
pub(crate) fn gpg_id_paths(root: &Path) -> Result<Vec<String>, Fault> {
    Ok(all_gpg_ids(root)?.into_iter().map(|(path, _)| path).collect())
}

/// The pinned keys, bindings and `.gpg-id` pins of one store.
pub struct Keyring {
    store: StoreDirs,
    keys: Vec<PinnedKey>,
    /// `(line, fingerprint)`: the line, trimmed, is satisfied by the key.
    bindings: Vec<(String, Fingerprint)>,
    /// Every `.gpg-id` as last accepted; `None` before setup recorded them.
    accepted: Option<Vec<Accepted>>,
}

fn failed(what: &str, e: impl std::fmt::Display) -> Fault {
    Fault::local(format!("could not {what} the pinned keys: {e}"))
}

fn key_bytes(key: &PublicKey) -> Result<Vec<u8>, Fault> {
    let mut bytes = Vec::new();
    key.to_writer_with_header(&mut bytes).map_err(|e| failed("save", e))?;
    Ok(bytes)
}

fn key_from_bytes(bytes: &[u8]) -> Option<PublicKey> {
    match PacketParser::new(bytes).next()? {
        Ok(Packet::PublicKey(key)) => Some(key),
        _ => None,
    }
}

fn b64(text: &str) -> String {
    B64.encode(text.as_bytes())
}

fn un_b64(text: &str) -> Option<String> {
    B64.decode(text).ok().and_then(|bytes| String::from_utf8(bytes).ok())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn file(store: &StoreDirs) -> std::path::PathBuf {
    store.state_dir().join(crate::files::layout::RECIPIENTS)
}

impl Keyring {
    /// The keyring of `store`; empty if nothing has been pinned yet. Its
    /// changes take the store's lock.
    pub fn open(store: &StoreDirs) -> Result<Self, Fault> {
        let bytes = read_bounded(&file(store), MAX_FILE_BYTES).map_err(|e| failed("read", e))?.unwrap_or_default();
        let text = String::from_utf8(bytes).map_err(|e| failed("read", e))?;
        let mut keyring = Self { store: store.clone(), keys: Vec::new(), bindings: Vec::new(), accepted: None };
        for line in text.lines() {
            let parts: Vec<&str> = line.split(' ').collect();
            match parts.as_slice() {
                ["key", _, "certificate", ..] => {}
                ["key", fpr, "card", _primary, _expires, _fingerprints, _emails, packet] => {
                    let damaged = || failed("read", format!("the key {fpr} is damaged"));
                    let fingerprint = Fingerprint::parse(fpr).ok_or_else(damaged)?;
                    let key = B64
                        .decode(packet)
                        .ok()
                        .and_then(|bytes| key_from_bytes(&bytes))
                        .filter(|key| Fingerprint::of(key) == fingerprint)
                        .ok_or_else(damaged)?;
                    keyring.keys.push(PinnedKey { fingerprint, key });
                }
                ["key", ..] => return Err(failed("read", "unknown key source")),
                ["accepted"] => {
                    keyring.accepted.get_or_insert_with(Vec::new);
                }
                ["bind", fpr, spec] => {
                    let damaged = || failed("read", "a damaged binding");
                    let spec = un_b64(spec).ok_or_else(damaged)?;
                    keyring.bindings.push((spec, Fingerprint::parse(fpr).ok_or_else(damaged)?));
                }
                ["gpgid", sha, path, text] => {
                    let damaged = || failed("read", "a damaged .gpg-id record");
                    let path = un_b64(path).ok_or_else(damaged)?;
                    let text = un_b64(text).ok_or_else(damaged)?;
                    keyring.accepted.get_or_insert_with(Vec::new).push(Accepted { path, sha: (*sha).to_owned(), text });
                }
                _ => {}
            }
        }
        Ok(keyring)
    }

    pub(crate) fn repo(&self) -> &Path {
        self.store.repo_dir()
    }

    fn save(&self, held: &Exclusive) -> Result<(), Fault> {
        debug_assert!(held.covers(self.store.lock()), "the keyring changed without its store's lock");
        let mut text = format!("{HEADER}\n");
        for key in &self.keys {
            text.push_str(&format!("key {} card - - - - {}\n", key.fingerprint, B64.encode(key_bytes(&key.key)?)));
        }
        for (spec, fpr) in &self.bindings {
            text.push_str(&format!("bind {fpr} {}\n", b64(spec)));
        }
        if let Some(accepted) = &self.accepted {
            text.push_str("accepted\n");
            for record in accepted {
                text.push_str(&format!("gpgid {} {} {}\n", record.sha, b64(&record.path), b64(&record.text)));
            }
        }
        write_atomic(&file(&self.store), text.as_bytes()).map_err(|e| failed("save", e))
    }

    pub fn keys(&self) -> &[PinnedKey] {
        &self.keys
    }

    /// Whether setup has recorded the store's recipients.
    pub(crate) fn confirmed(&self) -> bool {
        self.accepted.is_some()
    }

    /// Pins the card's decryption key (from its key session).
    /// Returns its fingerprint. Pinning it again replaces it.
    pub fn pin_card_key(&mut self, held: &Exclusive, key: &PublicKey) -> Result<Fingerprint, Fault> {
        let fingerprint = Fingerprint::of(key);
        self.keys.retain(|old| old.fingerprint != fingerprint);
        self.keys.push(PinnedKey { fingerprint: fingerprint.clone(), key: key.clone() });
        self.save(held)?;
        Ok(fingerprint)
    }

    /// Records that the `.gpg-id` line `line` is satisfied by the pinned key
    /// `fingerprint`, once setup's test decrypt has shown that the card
    /// opens entries written for that line.
    fn bind(&mut self, held: &Exclusive, line: &str, fingerprint: &Fingerprint) -> Result<(), Fault> {
        let line = line.trim().to_owned();
        self.bindings.retain(|(spec, _)| *spec != line);
        self.bindings.push((line, fingerprint.clone()));
        self.save(held)
    }

    /// The pinned key that satisfies `line`, and whether by a binding.
    /// Several keys named by one long key ID satisfy it no more than none.
    pub(crate) fn line_key(&self, line: &str) -> Option<(&PinnedKey, bool)> {
        let trimmed = line.trim();
        let bound = self
            .bindings
            .iter()
            .find(|(spec, _)| spec == trimmed)
            .and_then(|(_, fpr)| self.keys.iter().find(|key| key.fingerprint == *fpr));
        if let Some(key) = bound {
            return Some((key, true));
        }
        let spec = spec(trimmed);
        let mut named = self.keys.iter().filter(|key| spec.names(&key.fingerprint));
        match (named.next(), named.next()) {
            (Some(key), None) => Some((key, false)),
            _ => None,
        }
    }

    pub fn line_status(&self, line: &str) -> LineStatus {
        LineStatus::of(self.line_key(line))
    }

    /// How the recipients for entries in `folder` are satisfied, and
    /// whether they are the ones accepted. Reads the working copy; changes
    /// nothing.
    pub fn folder_recipients(&self, _reading: &Shared, folder: &str) -> Result<FolderRecipients, Fault> {
        let (gpg_id, bytes) = governing_gpg_id(self.repo(), folder)?;
        let text = gpg_id_text(&gpg_id, &bytes)?;
        let accepted = self.accepted.as_deref().and_then(|all| {
            let path = governing(folder, |file| all.iter().any(|a| a.path == file))?;
            all.iter().find(|a| a.path == path)
        });
        let changed = self.accepted.is_some()
            && accepted.is_none_or(|a| a.path != gpg_id || a.sha != sha256_hex(&bytes));
        let lines: Vec<RecipientLine> = parse_gpg_id(text)
            .into_iter()
            .map(|line| RecipientLine { status: self.line_status(&line), line })
            .collect();
        let confirmed = self.confirmed();
        let ready = lines.iter().all(|l| matches!(l.status, LineStatus::Ready { .. }));
        Ok(FolderRecipients {
            gpg_id,
            writable: confirmed && !changed && ready,
            lines,
            confirmed,
            changed,
            accepted_gpg_id: accepted.map(|a| a.path.clone()),
            accepted_lines: accepted.map(|a| parse_gpg_id(&a.text)).unwrap_or_default(),
        })
    }

    /// The keys to encrypt to for an entry in `folder`: one per distinct
    /// key, in `.gpg-id` order. Refused unless the recipients are the
    /// accepted ones and every line is satisfied; every unsatisfied line is
    /// named. Accepts nothing by itself.
    pub fn resolve(&self, reading: &Shared, folder: &str) -> Result<Vec<PublicKey>, WriteError> {
        let recipients = self.folder_recipients(reading, folder)?;
        if !recipients.confirmed {
            return Err(WriteError::RecipientsNotConfirmed);
        }
        if recipients.changed {
            return Err(WriteError::RecipientsChanged {
                gpg_id: recipients.gpg_id,
                before: recipients.accepted_lines,
                after: recipients.lines.into_iter().map(|l| l.line).collect(),
            });
        }
        let mut missing = Vec::new();
        let mut keys: Vec<PublicKey> = Vec::new();
        for RecipientLine { line, .. } in &recipients.lines {
            match self.line_key(line) {
                Some((key, _)) => {
                    if !keys.iter().any(|k| Fingerprint::of(k) == key.fingerprint) {
                        keys.push(key.key.clone());
                    }
                }
                None => missing.push(line.clone()),
            }
        }
        if recipients.lines.is_empty() {
            missing.push(format!("{} names no key", recipients.gpg_id));
        }
        if !missing.is_empty() {
            return Err(WriteError::MissingRecipients { gpg_id: recipients.gpg_id, missing });
        }
        Ok(keys)
    }

    /// Setup: records every `.gpg-id` in the store as accepted, replacing
    /// any earlier record. The only acceptance that is not about one folder
    /// the person reviewed; call it once the store is cloned and shown. One
    /// that is not text is left out: nothing can be saved under it.
    pub fn accept_all_recipients_at_setup(&mut self, held: &Exclusive) -> Result<(), Fault> {
        let all = all_gpg_ids(self.repo())?
            .into_iter()
            .filter_map(|(path, bytes)| {
                let text = gpg_id_text(&path, &bytes).ok()?.to_owned();
                Some(Accepted { sha: sha256_hex(&bytes), text, path })
            })
            .collect();
        self.accepted = Some(all);
        self.save(held)
    }

    /// Accepts the recipients `folder` has now, after the person has seen
    /// what changed: the `.gpg-id` that governs it now is recorded, and any
    /// accepted one between it and the folder that no longer exists is
    /// dropped. Other folders' records are left as they are.
    pub fn accept_recipients(&mut self, held: &Exclusive, folder: &str) -> Result<(), WriteError> {
        let (path, bytes) = governing_gpg_id(self.repo(), folder)?;
        let Some(accepted) = self.accepted.as_mut() else {
            return Err(WriteError::RecipientsNotConfirmed);
        };
        let governing_dir = folder_of_gpg_id(&path);
        // `.gpg-id` files in the folder or above it, below the governing one:
        // they were removed, or the governing one would be among them.
        for at in folder_and_parents(folder).take_while(|at| *at != governing_dir && !at.is_empty()) {
            let gone = gpg_id_in(at);
            accepted.retain(|a| a.path != gone);
        }
        let text = gpg_id_text(&path, &bytes)?.to_owned();
        accepted.retain(|a| a.path != path);
        accepted.push(Accepted { sha: sha256_hex(&bytes), text, path });
        Ok(self.save(held)?)
    }

    /// After setup's test decrypt opened `entry` (a path in the store,
    /// with or without `.gpg`) whose bytes are `ciphertext`: binds the card's
    /// key `card_fingerprint` (just pinned) to the one line of the
    /// `.gpg-id` that governs that entry which no pinned key satisfies.
    /// Binds nothing, and returns no lines, unless the entry's own recipient
    /// packets name the card's key (a hidden recipient proves nothing about
    /// which line is the card's), no line of that `.gpg-id` already means
    /// the card, and exactly one line is open: with several, nothing says
    /// which is the card's, and saving there stays refused. Lines of
    /// other `.gpg-id` files are never bound: a top-level line naming a
    /// team key is not the card's because a folder below names the owner.
    pub fn bind_card_for_opened_entry(
        &mut self,
        held: &Exclusive,
        card_fingerprint: &Fingerprint,
        entry: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<String>, Fault> {
        let names_card = crate::recipients::entry_recipients(ciphertext)
            .is_ok_and(|found| crate::recipients::names_key(&found.keys, card_fingerprint));
        if !names_card {
            return Ok(Vec::new());
        }
        let folder = folder_of(entry);
        let recipients = self.folder_recipients(held, folder)?;
        self.bind_one_open_line(held, recipients, card_fingerprint)
    }

    /// Binds the card to the one open line of `recipients`, unless a line
    /// already means it; see [`Self::bind_card_for_opened_entry`].
    fn bind_one_open_line(
        &mut self,
        held: &Exclusive,
        recipients: FolderRecipients,
        card_fingerprint: &Fingerprint,
    ) -> Result<Vec<String>, Fault> {
        // A line already means the card's key (it names it, or was bound
        // before): every line still open is someone else's, and binding one
        // of them to the card would encrypt to fewer keys than `.gpg-id`
        // names.
        let card_has_a_line = recipients
            .lines
            .iter()
            .any(|line| matches!(&line.status, LineStatus::Ready { fingerprint, .. } if fingerprint == card_fingerprint.as_str()));
        if card_has_a_line {
            return Ok(Vec::new());
        }
        let open: Vec<String> = recipients
            .lines
            .iter()
            .filter(|line| !matches!(line.status, LineStatus::Ready { .. }))
            .map(|line| line.line.clone())
            .collect();
        match open.as_slice() {
            [line] => {
                self.bind(held, line, card_fingerprint)?;
                Ok(open)
            }
            _ => Ok(Vec::new()),
        }
    }
}

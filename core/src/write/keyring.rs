//! The public keys paw encrypts to, pinned on this device.
//!
//! `.gpg-id` names recipients by fingerprint, key ID or e-mail address,
//! but encrypting needs their public keys, and the repository is not a
//! trusted place to get them from. So the keys live here, in the store's
//! state directory (not in the git repository), and only get here two
//! ways: the card's own decryption key, pinned at setup, and certificates
//! the person imports (see [`super::cert`] for what is checked).
//!
//! A `.gpg-id` line is satisfied by:
//! 1. a binding: an explicit record that this line means this pinned key.
//!    Setup makes one after the test decrypt, because the owner's
//!    `.gpg-id` usually names the primary key's fingerprint while the card
//!    exposes only its encryption subkey, and nothing on the phone can
//!    prove the two belong together except the person and that decrypt;
//! 2. otherwise, exactly one pinned key that the line names: the full
//!    fingerprint or long key ID of the key itself or of its certificate's
//!    verified primary key, or an e-mail address in one of its verified
//!    user IDs (exact, any letter case; GnuPG would also accept a part of a
//!    user ID, paw does not). Any other subkey of the certificate (older,
//!    revoked, expired, not for encryption, or not vouched for by the
//!    primary key) never satisfies a line: the line is refused with the
//!    reason, and only an explicit binding can make it mean a pinned key.
//!
//! Short key IDs and names are never matched by themselves: they need a
//! binding. Resolving a folder is all or nothing: if any line of its
//! `.gpg-id` is not satisfied, or a key has expired, saving there is
//! refused with every such line named. paw never encrypts to fewer keys
//! than `.gpg-id` names.
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
//! secret.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use pgp::packet::{Packet, PacketParser, PacketTrait, PublicKey};
use sha2::{Digest, Sha256};

use super::WriteError;
use super::cert::{CertificateInfo, email_in, fingerprint_of};
use crate::recipients::{hex, key_id};
use crate::path::{folder_and_parents, folder_of, folder_of_gpg_id, gpg_id_in, governing};
use crate::store::gpg_id::{governing_gpg_id, key_digits, parse_gpg_id};
use crate::store::read::read_file;
use crate::store::state_file::{read_bounded, write_atomic};

const FILE: &str = crate::layout::RECIPIENTS;
const HEADER: &str = "# paw pinned recipients, version 1";
const TEMP_PREFIX: &str = ".recipients.tmp-";

/// Where a pinned key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum KeySource {
    /// The plugged-in card's decryption key, pinned at setup.
    Card,
    /// An imported certificate.
    Certificate,
}

/// A public key paw may encrypt to.
#[derive(Clone)]
pub struct PinnedKey {
    /// The fingerprint of the key itself, upper-case hex.
    pub fingerprint: String,
    pub source: KeySource,
    /// For a certificate: its primary key's fingerprint.
    pub primary: Option<String>,
    /// The certificate's fingerprints a line may name it by: its primary
    /// key's (and the key's own, which is always allowed).
    pub fingerprints: Vec<String>,
    /// The certificate's other subkeys, with why paw does not use them.
    pub refused: Vec<(String, String)>,
    /// E-mail addresses from its verified user IDs.
    pub emails: Vec<String>,
    /// When it stops being usable, seconds since 1970.
    pub expires: Option<u32>,
    pub key: PublicKey,
}

impl std::fmt::Debug for PinnedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PinnedKey({} {:?})", self.fingerprint, self.source)
    }
}

/// What one `.gpg-id` line looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Spec {
    /// 40 (v4) or 64 (v6) hex digits.
    Fingerprint(String),
    /// 16 hex digits.
    LongId(String),
    /// An e-mail address, lower case.
    Email(String),
    /// A short key ID, a name or anything else: only a binding helps.
    Other,
}

fn spec(line: &str) -> Spec {
    if let Some(digits) = key_digits(line) {
        return match digits.len() {
            40 | 64 => Spec::Fingerprint(digits),
            16 => Spec::LongId(digits),
            _ => Spec::Other,
        };
    }
    let line = line.trim();
    let bare = line.strip_suffix('!').unwrap_or(line);
    if bare.contains('@') {
        let address = bare.strip_prefix('=').unwrap_or(bare);
        if let Some(email) = email_in(address) {
            return Spec::Email(email);
        }
    }
    Spec::Other
}

impl PinnedKey {
    /// The key paw would pin for the certificate `info`: its chosen
    /// encryption key.
    fn of_certificate(info: &CertificateInfo) -> Result<Self, WriteError> {
        let choice = info.encryption.as_ref().map_err(|why| WriteError::NoEncryptionKey {
            fingerprint: info.primary.clone(),
            reason: why.clone(),
        })?;
        Ok(Self {
            fingerprint: choice.fingerprint.clone(),
            source: KeySource::Certificate,
            primary: Some(info.primary.clone()),
            fingerprints: info.fingerprints.clone(),
            refused: info.others.clone(),
            emails: info.emails.clone(),
            expires: choice.expires,
            key: choice.key.clone(),
        })
    }

    fn all_fingerprints(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.fingerprint.as_str()).chain(self.fingerprints.iter().map(String::as_str))
    }

    /// Whether `line` names this key by itself (no binding).
    fn named_by(&self, line: &str) -> bool {
        match spec(line) {
            Spec::Fingerprint(fpr) => self.all_fingerprints().any(|own| own == fpr),
            Spec::LongId(id) => self.all_fingerprints().any(|own| key_id(own) == Some(id.as_str())),
            Spec::Email(email) => self.emails.contains(&email),
            Spec::Other => false,
        }
    }

    /// If `line` names one of the certificate's other subkeys, why that
    /// subkey is not used.
    fn refusal(&self, line: &str) -> Option<&str> {
        let names = |fpr: &str| match spec(line) {
            Spec::Fingerprint(wanted) => wanted == fpr,
            Spec::LongId(id) => key_id(fpr) == Some(id.as_str()),
            _ => false,
        };
        self.refused.iter().find(|(fpr, _)| names(fpr)).map(|(_, why)| why.as_str())
    }

    fn expired(&self, now: u32) -> bool {
        self.expires.is_some_and(|end| end <= now)
    }
}

/// How one `.gpg-id` line is satisfied, for the screen that shows which
/// keys can open a folder.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum LineStatus {
    /// Encrypting uses the pinned key with this fingerprint.
    Ready { fingerprint: String, source: KeySource, bound: bool },
    /// No pinned key matches: import the recipient's public key.
    Missing,
    /// Several pinned keys match; bind the line to one.
    Ambiguous { fingerprints: Vec<String> },
    /// The key it resolves to has expired; import a refreshed one.
    Expired { fingerprint: String },
    /// It names a subkey of the certificate pinned as `fingerprint` that
    /// paw does not encrypt to, for `reason`. Binding the line to a pinned
    /// key is the only way to use it.
    Refused { fingerprint: String, subkey: String, reason: String },
}

/// One line of a `.gpg-id` and how it is satisfied.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RecipientLine {
    pub line: String,
    pub status: LineStatus,
}

/// The recipients for one folder.
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
fn all_gpg_ids(root: &Path) -> Result<Vec<(String, Vec<u8>)>, WriteError> {
    let git = |e: git2::Error| WriteError::Failed { message: format!("could not read the store: {}", e.message()) };
    let repo = git2::Repository::open(root).map_err(git)?;
    let tree = repo.head().and_then(|h| h.peel_to_tree()).map_err(git)?;
    let mut found = Vec::new();
    tree.walk(git2::TreeWalkMode::PreOrder, |folder, entry| {
        let name = entry.name().unwrap_or_default();
        if entry.kind() == Some(git2::ObjectType::Tree) {
            // Hidden folders (`.git`-like, `.extensions`) are not the store's.
            return if name.starts_with('.') { git2::TreeWalkResult::Skip } else { git2::TreeWalkResult::Ok };
        }
        if name == ".gpg-id"
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
pub(crate) fn gpg_id_paths(root: &Path) -> Result<Vec<String>, WriteError> {
    Ok(all_gpg_ids(root)?.into_iter().map(|(path, _)| path).collect())
}

/// Every key line of every `.gpg-id` committed in the store at `root`, as
/// it is in the working copy: sorted, each once. A store with nothing
/// committed yet has none, and a `.gpg-id` that can't be read adds none.
fn all_gpg_id_lines(root: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    for path in gpg_id_paths(root).unwrap_or_default() {
        if let Ok(Some(bytes)) = read_file(root, &path) {
            lines.extend(parse_gpg_id(&String::from_utf8_lossy(&bytes)));
        }
    }
    lines.sort();
    lines.dedup();
    lines
}

/// A public certificate, as the import screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CertificateSummary {
    pub primary_fingerprint: String,
    pub user_ids: Vec<String>,
    /// The key paw would encrypt to; `None` if there is none usable.
    pub encryption_fingerprint: Option<String>,
    /// When that key expires, seconds since 1970.
    pub expires: Option<i64>,
    /// Why there is no usable key.
    pub problem: Option<String>,
    /// The `.gpg-id` lines of this store it satisfies.
    pub satisfies: Vec<String>,
}

/// How each certificate in `infos` stands with the store at `root`.
pub fn certificate_summaries(root: &Path, infos: &[CertificateInfo]) -> Vec<CertificateSummary> {
    let lines = all_gpg_id_lines(root);
    infos
        .iter()
        .map(|info| CertificateSummary {
            primary_fingerprint: info.primary.clone(),
            user_ids: info.user_ids.clone(),
            encryption_fingerprint: info.encryption.as_ref().ok().map(|c| c.fingerprint.clone()),
            expires: info.encryption.as_ref().ok().and_then(|c| c.expires).map(i64::from),
            problem: info.encryption.as_ref().err().cloned(),
            satisfies: lines.iter().filter(|line| certificate_named_by(info, line)).cloned().collect(),
        })
        .collect()
}

/// The pinned keys, bindings and `.gpg-id` pins of one store.
pub struct Keyring {
    file: PathBuf,
    keys: Vec<PinnedKey>,
    /// `(line, fingerprint)`: the line, trimmed, is satisfied by the key.
    bindings: Vec<(String, String)>,
    /// Every `.gpg-id` as last accepted; `None` before setup recorded them.
    accepted: Option<Vec<Accepted>>,
}

fn failed(what: &str, e: impl std::fmt::Display) -> WriteError {
    WriteError::Failed { message: format!("could not {what} the pinned keys: {e}") }
}

fn key_bytes(key: &PublicKey) -> Result<Vec<u8>, WriteError> {
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

fn list(items: &[String]) -> String {
    if items.is_empty() { "-".into() } else { items.join(",") }
}

fn unlist(text: &str) -> Vec<String> {
    if text == "-" { Vec::new() } else { text.split(',').map(str::to_owned).collect() }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

impl Keyring {
    /// The keyring of the store whose state directory is `state_dir`.
    /// Empty if nothing has been pinned yet.
    pub fn open(state_dir: &Path) -> Result<Self, WriteError> {
        let file = state_dir.join(FILE);
        let bytes = read_bounded(&file).map_err(|e| failed("read", e))?.unwrap_or_default();
        let text = String::from_utf8(bytes).map_err(|e| failed("read", e))?;
        let mut keyring = Self { file, keys: Vec::new(), bindings: Vec::new(), accepted: None };
        for line in text.lines() {
            let parts: Vec<&str> = line.split(' ').collect();
            match parts.as_slice() {
                ["key", fpr, source, primary, expires, fprs, emails, packet] => {
                    let source = match *source {
                        "card" => KeySource::Card,
                        "certificate" => KeySource::Certificate,
                        _ => return Err(failed("read", "unknown key source")),
                    };
                    let key = B64
                        .decode(packet)
                        .ok()
                        .and_then(|bytes| key_from_bytes(&bytes))
                        .filter(|key| fingerprint_of(key) == *fpr)
                        .ok_or_else(|| failed("read", format!("the key {fpr} is damaged")))?;
                    keyring.keys.push(PinnedKey {
                        fingerprint: (*fpr).to_owned(),
                        source,
                        primary: (*primary != "-").then(|| (*primary).to_owned()),
                        fingerprints: unlist(fprs),
                        refused: Vec::new(),
                        emails: unlist(emails),
                        expires: expires.parse().ok(),
                        key,
                    });
                }
                ["sub", fpr, subkey, reason] => {
                    let reason = un_b64(reason).ok_or_else(|| failed("read", "a damaged subkey note"))?;
                    if let Some(key) = keyring.keys.iter_mut().find(|k| k.fingerprint == *fpr) {
                        key.refused.push(((*subkey).to_owned(), reason));
                    }
                }
                ["accepted"] => {
                    keyring.accepted.get_or_insert_with(Vec::new);
                }
                ["bind", fpr, spec] => {
                    let spec = un_b64(spec).ok_or_else(|| failed("read", "a damaged binding"))?;
                    keyring.bindings.push((spec, (*fpr).to_owned()));
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

    fn save(&self) -> Result<(), WriteError> {
        let mut text = format!("{HEADER}\n");
        for key in &self.keys {
            let source = match key.source {
                KeySource::Card => "card",
                KeySource::Certificate => "certificate",
            };
            text.push_str(&format!(
                "key {} {source} {} {} {} {} {}\n",
                key.fingerprint,
                key.primary.as_deref().unwrap_or("-"),
                key.expires.map_or("-".into(), |e| e.to_string()),
                list(&key.fingerprints),
                list(&key.emails),
                B64.encode(key_bytes(&key.key)?),
            ));
            for (subkey, reason) in &key.refused {
                text.push_str(&format!("sub {} {subkey} {}\n", key.fingerprint, b64(reason)));
            }
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
        let dir = self.file.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| failed("save", e))?;
        write_atomic(dir, TEMP_PREFIX, &self.file, text.as_bytes()).map_err(|e| failed("save", e))
    }

    /// Every pinned key.
    pub fn keys(&self) -> &[PinnedKey] {
        &self.keys
    }

    fn pin(&mut self, key: PinnedKey) -> Result<String, WriteError> {
        let fingerprint = key.fingerprint.clone();
        self.keys.retain(|old| old.fingerprint != fingerprint);
        self.keys.push(key);
        self.save()?;
        Ok(fingerprint)
    }

    /// Pins the card's decryption key (from its key session).
    /// Returns its fingerprint. Pinning it again replaces it.
    pub fn pin_card_key(&mut self, key: &PublicKey) -> Result<String, WriteError> {
        let fingerprint = fingerprint_of(key);
        self.pin(PinnedKey {
            fingerprint,
            source: KeySource::Card,
            primary: None,
            fingerprints: Vec::new(),
            refused: Vec::new(),
            emails: Vec::new(),
            expires: None,
            key: key.clone(),
        })
    }

    /// Pins the encryption key chosen from an imported certificate.
    pub fn pin_certificate(&mut self, info: &CertificateInfo) -> Result<String, WriteError> {
        self.pin(PinnedKey::of_certificate(info)?)
    }

    /// Records that the `.gpg-id` line `line` is satisfied by the pinned key
    /// `fingerprint`. For setup, once the test decrypt has shown that the
    /// card opens entries written for that line; or when the person picks a
    /// key for a line that names it by short ID or by name.
    pub fn bind(&mut self, line: &str, fingerprint: &str) -> Result<(), WriteError> {
        let fingerprint = fingerprint.trim().to_ascii_uppercase();
        if !self.keys.iter().any(|key| key.fingerprint == fingerprint) {
            return Err(WriteError::NotPinned { fingerprint });
        }
        let line = line.trim().to_owned();
        self.bindings.retain(|(spec, _)| *spec != line);
        self.bindings.push((line, fingerprint));
        self.save()
    }

    /// How `line` is satisfied at `now` (seconds since 1970).
    pub fn line_status(&self, line: &str, now: u32) -> LineStatus {
        let trimmed = line.trim();
        let bound = self
            .bindings
            .iter()
            .find(|(spec, _)| spec == trimmed)
            .and_then(|(_, fpr)| self.keys.iter().find(|key| key.fingerprint == *fpr));
        let (key, bound) = match bound {
            Some(key) => (key, true),
            None => {
                // A key named by its own full fingerprint is that key, even
                // if another certificate lists the same fingerprint.
                let exact = match spec(trimmed) {
                    Spec::Fingerprint(fpr) => self.keys.iter().find(|key| key.fingerprint == fpr),
                    _ => None,
                };
                let named: Vec<&PinnedKey> = match exact {
                    Some(key) => vec![key],
                    None => self.keys.iter().filter(|key| key.named_by(trimmed)).collect(),
                };
                match named.as_slice() {
                    [] => {
                        return self
                            .keys
                            .iter()
                            .find_map(|key| {
                                let reason = key.refusal(trimmed)?;
                                let subkey = key.refused.iter().find(|(_, why)| why == reason).map(|(f, _)| f.clone())?;
                                Some(LineStatus::Refused { fingerprint: key.fingerprint.clone(), subkey, reason: reason.to_owned() })
                            })
                            .unwrap_or(LineStatus::Missing);
                    }
                    [key] => (*key, false),
                    several => {
                        return LineStatus::Ambiguous {
                            fingerprints: several.iter().map(|key| key.fingerprint.clone()).collect(),
                        };
                    }
                }
            }
        };
        if key.expired(now) {
            return LineStatus::Expired { fingerprint: key.fingerprint.clone() };
        }
        LineStatus::Ready { fingerprint: key.fingerprint.clone(), source: key.source, bound }
    }

    /// How the recipients for entries in `folder` of the store at `root`
    /// are satisfied, and whether they are the ones accepted. Reads the
    /// working copy; changes nothing.
    pub fn folder_recipients(&self, root: &Path, folder: &str, now: u32) -> Result<FolderRecipients, WriteError> {
        let (gpg_id, bytes) = governing_gpg_id(root, folder)?;
        let text = String::from_utf8(bytes.clone())
            .map_err(|_| WriteError::Failed { message: format!("{gpg_id} is not a text file") })?;
        let accepted = self.accepted.as_deref().and_then(|all| {
            let path = governing(folder, |file| all.iter().any(|a| a.path == file))?;
            all.iter().find(|a| a.path == path)
        });
        let changed = self.accepted.is_some()
            && accepted.is_none_or(|a| a.path != gpg_id || a.sha != sha256_hex(&bytes));
        let lines: Vec<RecipientLine> = parse_gpg_id(&text)
            .into_iter()
            .map(|line| RecipientLine { status: self.line_status(&line, now), line })
            .collect();
        let confirmed = self.accepted.is_some();
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
    pub fn resolve(&self, root: &Path, folder: &str, now: u32) -> Result<Vec<PublicKey>, WriteError> {
        let recipients = self.folder_recipients(root, folder, now)?;
        if !recipients.confirmed {
            return Err(WriteError::RecipientsNotConfirmed);
        }
        if recipients.changed {
            return Err(WriteError::RecipientsChanged {
                gpg_id: recipients.gpg_id,
                before_gpg_id: recipients.accepted_gpg_id,
                before: recipients.accepted_lines,
                after: recipients.lines.into_iter().map(|l| l.line).collect(),
            });
        }
        let mut missing = Vec::new();
        let mut keys: Vec<PublicKey> = Vec::new();
        for RecipientLine { line, status } in &recipients.lines {
            match status {
                LineStatus::Ready { fingerprint, .. } => {
                    let Some(key) = self.keys.iter().find(|key| key.fingerprint == *fingerprint) else {
                        missing.push(line.clone());
                        continue;
                    };
                    if !keys.iter().any(|k| fingerprint_of(k) == *fingerprint) {
                        keys.push(key.key.clone());
                    }
                }
                LineStatus::Missing => missing.push(line.clone()),
                LineStatus::Ambiguous { .. } => missing.push(format!("{line} (more than one key matches)")),
                LineStatus::Expired { .. } => missing.push(format!("{line} (its key has expired)")),
                LineStatus::Refused { reason, .. } => {
                    missing.push(format!("{line} (it names a subkey paw does not encrypt to: {reason})"))
                }
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
    /// the person reviewed; call it once the store is cloned and shown.
    pub fn accept_all_recipients_at_setup(&mut self, root: &Path) -> Result<(), WriteError> {
        let all = all_gpg_ids(root)?
            .into_iter()
            .map(|(path, bytes)| Accepted { sha: sha256_hex(&bytes), text: String::from_utf8_lossy(&bytes).into_owned(), path })
            .collect();
        self.accepted = Some(all);
        self.save()
    }

    /// Accepts the recipients `folder` has now, after the person has seen
    /// what changed: the `.gpg-id` that governs it now is recorded, and any
    /// accepted one between it and the folder that no longer exists is
    /// dropped. Other folders' records are left as they are.
    pub fn accept_recipients(&mut self, root: &Path, folder: &str) -> Result<(), WriteError> {
        let (path, bytes) = governing_gpg_id(root, folder)?;
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
        accepted.retain(|a| a.path != path);
        accepted.push(Accepted { sha: sha256_hex(&bytes), text: String::from_utf8_lossy(&bytes).into_owned(), path });
        self.save()
    }

    /// After setup's test decrypt opened `entry` (a path in the store,
    /// with or without `.gpg`) whose bytes are `ciphertext`: binds the card's
    /// key `card_fingerprint` (already pinned) to the one line of the
    /// `.gpg-id` that governs that entry which no pinned key satisfies.
    /// Binds nothing, and returns no lines, unless the entry's own recipient
    /// packets name the card's key (a hidden recipient proves nothing about
    /// which line is the card's) and no line of that `.gpg-id` already
    /// means the card. If several lines are open, the person has to choose
    /// ([`Self::bind`]); that is [`WriteError::MissingRecipients`]. Lines of
    /// other `.gpg-id` files are never bound: a top-level line naming a
    /// team key is not the card's because a folder below names the owner.
    pub fn bind_card_for_opened_entry(
        &mut self,
        root: &Path,
        card_fingerprint: &str,
        entry: &str,
        ciphertext: &[u8],
        now: u32,
    ) -> Result<Vec<String>, WriteError> {
        let card_fingerprint = card_fingerprint.trim().to_ascii_uppercase();
        if !self.keys.iter().any(|k| k.fingerprint == card_fingerprint && k.source == KeySource::Card) {
            return Err(WriteError::NotPinned { fingerprint: card_fingerprint });
        }
        let names_card = crate::recipients::entry_recipients(ciphertext)
            .is_ok_and(|found| crate::recipients::names_key(&found.keys, &card_fingerprint));
        if !names_card {
            return Ok(Vec::new());
        }
        let folder = folder_of(entry);
        let recipients = self.folder_recipients(root, folder, now)?;
        self.bind_one_open_line(recipients, &card_fingerprint)
    }

    /// Binds the card to the one open line of `recipients`, unless a line
    /// already means it; see [`Self::bind_card_for_opened_entry`].
    fn bind_one_open_line(
        &mut self,
        recipients: FolderRecipients,
        card_fingerprint: &str,
    ) -> Result<Vec<String>, WriteError> {
        // A line already means the card's key (it names it, or was bound
        // before): every line still open is someone else's, and binding one
        // of them to the card would encrypt to fewer keys than `.gpg-id`
        // names.
        let card_has_a_line = recipients
            .lines
            .iter()
            .any(|line| matches!(&line.status, LineStatus::Ready { fingerprint, .. } if *fingerprint == card_fingerprint));
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
            [] => Ok(Vec::new()),
            [line] => {
                self.bind(line, card_fingerprint)?;
                Ok(open)
            }
            _ => Err(WriteError::MissingRecipients { gpg_id: recipients.gpg_id, missing: open }),
        }
    }
}

/// Whether `line` names the certificate `info` by itself (its primary or
/// chosen encryption key's fingerprint or long key ID, or an e-mail
/// address), as it would once pinned.
pub fn certificate_named_by(info: &CertificateInfo, line: &str) -> bool {
    PinnedKey::of_certificate(info).is_ok_and(|key| key.named_by(line))
}

#[cfg(test)]
mod tests {
    use super::all_gpg_id_lines;

    fn commit(repo: &git2::Repository, files: &[(&str, &str)]) {
        let mut index = repo.index().unwrap();
        for (path, text) in files {
            let file = repo.workdir().unwrap().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, text).unwrap();
            index.add_path(std::path::Path::new(path)).unwrap();
        }
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let who = git2::Signature::now("t", "t@example.com").unwrap();
        repo.commit(Some("HEAD"), &who, &who, "files", &tree, &[]).unwrap();
    }

    #[test]
    fn a_store_with_nothing_committed_has_no_lines() {
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        assert!(all_gpg_id_lines(dir.path()).is_empty());
    }

    #[test]
    fn a_gpg_id_that_cannot_be_read_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        commit(&repo, &[(".gpg-id", "AAAA1111\n"), ("work/.gpg-id", "BBBB2222\n")]);
        std::fs::remove_file(dir.path().join("work/.gpg-id")).unwrap();
        std::os::unix::fs::symlink("../.gpg-id", dir.path().join("work/.gpg-id")).unwrap();
        assert_eq!(all_gpg_id_lines(dir.path()), ["AAAA1111"]);
    }
}

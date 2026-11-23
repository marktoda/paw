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
//! 2. otherwise, exactly one pinned key that the line names: a full
//!    fingerprint of the key, of its certificate's primary key or of any of
//!    its subkeys; a long key ID of any of those; or an e-mail address in
//!    one of its verified user IDs (exact, any letter case; GnuPG would
//!    also accept a part of a user ID, paw does not).
//!
//! Short key IDs and names are never matched by themselves: they need a
//! binding. Resolving a folder is all or nothing: if any line of its
//! `.gpg-id` is not satisfied, or a key has expired, saving there is
//! refused with every such line named. paw never encrypts to fewer keys
//! than `.gpg-id` names.
//!
//! Each `.gpg-id` file's SHA-256 is pinned the first time paw encrypts for
//! it. If it later differs (a pull changed who can open the folder),
//! saving there is refused until the person accepts the new list.
//!
//! The file is small text, written whole by rename. Nothing in it is
//! secret.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use pgp::packet::{Packet, PacketParser, PacketTrait, PublicKey};
use sha2::{Digest, Sha256};

use super::WriteError;
use super::cert::{CertificateInfo, email_in, fingerprint_of, hex};
use crate::store::gpg_id::{governing_gpg_id, parse_gpg_id};

const FILE: &str = "recipients";
const HEADER: &str = "# paw pinned recipients, version 1";

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
    /// Every fingerprint of the certificate (primary and subkeys).
    pub fingerprints: Vec<String>,
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
    let line = line.trim();
    // `!` asks GnuPG for exactly this subkey; it names the same key.
    let bare = line.strip_suffix('!').unwrap_or(line);
    let compact: String = bare.chars().filter(|c| !c.is_whitespace()).collect();
    let digits = compact.strip_prefix("0x").or_else(|| compact.strip_prefix("0X")).unwrap_or(&compact);
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_hexdigit()) {
        let digits = digits.to_ascii_uppercase();
        return match digits.len() {
            40 | 64 => Spec::Fingerprint(digits),
            16 => Spec::LongId(digits),
            _ => Spec::Other,
        };
    }
    if bare.contains('@') {
        let address = bare.strip_prefix('=').unwrap_or(bare);
        if let Some(email) = email_in(address) {
            return Spec::Email(email);
        }
    }
    Spec::Other
}

/// The key ID of a fingerprint: the low 64 bits for v4, the high for v6.
fn key_id(fingerprint: &str) -> Option<&str> {
    match fingerprint.len() {
        40 => fingerprint.get(24..),
        64 => fingerprint.get(..16),
        _ => None,
    }
}

impl PinnedKey {
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
    /// The `.gpg-id` changed since paw last encrypted for it, and the
    /// change has not been accepted.
    pub changed: bool,
}

impl FolderRecipients {
    /// Whether paw can save in the folder now.
    pub fn writable(&self) -> bool {
        !self.changed && self.lines.iter().all(|l| matches!(l.status, LineStatus::Ready { .. }))
    }
}

/// The pinned keys, bindings and `.gpg-id` pins of one store.
pub struct Keyring {
    file: PathBuf,
    keys: Vec<PinnedKey>,
    /// `(line, fingerprint)`: the line, trimmed, is satisfied by the key.
    bindings: Vec<(String, String)>,
    /// `(path of a .gpg-id, SHA-256 hex of its bytes)`.
    gpg_ids: Vec<(String, String)>,
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
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(failed("read", e)),
        };
        let mut keyring = Self { file, keys: Vec::new(), bindings: Vec::new(), gpg_ids: Vec::new() };
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
                        emails: unlist(emails),
                        expires: expires.parse().ok(),
                        key,
                    });
                }
                ["bind", fpr, spec] => {
                    let spec = un_b64(spec).ok_or_else(|| failed("read", "a damaged binding"))?;
                    keyring.bindings.push((spec, (*fpr).to_owned()));
                }
                ["gpgid", sha, path] => {
                    let path = un_b64(path).ok_or_else(|| failed("read", "a damaged .gpg-id pin"))?;
                    keyring.gpg_ids.push((path, (*sha).to_owned()));
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
        }
        for (spec, fpr) in &self.bindings {
            text.push_str(&format!("bind {fpr} {}\n", b64(spec)));
        }
        for (path, sha) in &self.gpg_ids {
            text.push_str(&format!("gpgid {sha} {}\n", b64(path)));
        }
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| failed("save", e))?;
        }
        let temporary = self.file.with_extension("tmp");
        std::fs::write(&temporary, text).map_err(|e| failed("save", e))?;
        std::fs::rename(&temporary, &self.file).map_err(|e| failed("save", e))
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

    /// Pins the card's decryption key (from [`crate::card_decryption_key`]).
    /// Returns its fingerprint. Pinning it again replaces it.
    pub fn pin_card_key(&mut self, key: &PublicKey) -> Result<String, WriteError> {
        let fingerprint = fingerprint_of(key);
        self.pin(PinnedKey {
            fingerprint,
            source: KeySource::Card,
            primary: None,
            fingerprints: Vec::new(),
            emails: Vec::new(),
            expires: None,
            key: key.clone(),
        })
    }

    /// Pins the encryption key chosen from an imported certificate.
    pub fn pin_certificate(&mut self, info: &CertificateInfo) -> Result<String, WriteError> {
        let choice = info.encryption.as_ref().map_err(|why| WriteError::NoEncryptionKey {
            fingerprint: info.primary.clone(),
            reason: why.clone(),
        })?;
        self.pin(PinnedKey {
            fingerprint: choice.fingerprint.clone(),
            source: KeySource::Certificate,
            primary: Some(info.primary.clone()),
            fingerprints: info.fingerprints.clone(),
            emails: info.emails.clone(),
            expires: choice.expires,
            key: choice.key.clone(),
        })
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

    /// Forgets a pinned key and every binding to it.
    pub fn forget(&mut self, fingerprint: &str) -> Result<(), WriteError> {
        self.keys.retain(|key| key.fingerprint != fingerprint);
        self.bindings.retain(|(_, fpr)| fpr != fingerprint);
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
                    [] => return LineStatus::Missing,
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
    /// are satisfied. Reads the working copy; changes nothing.
    pub fn folder_recipients(&self, root: &Path, folder: &str, now: u32) -> Result<FolderRecipients, WriteError> {
        let (gpg_id, bytes) = governing_gpg_id(root, folder)?;
        let text = String::from_utf8(bytes.clone())
            .map_err(|_| WriteError::Failed { message: format!("{gpg_id} is not a text file") })?;
        let changed = self
            .gpg_ids
            .iter()
            .any(|(path, sha)| *path == gpg_id && *sha != sha256_hex(&bytes));
        let lines = parse_gpg_id(&text)
            .into_iter()
            .map(|line| RecipientLine { status: self.line_status(&line, now), line })
            .collect();
        Ok(FolderRecipients { gpg_id, lines, changed })
    }

    /// The keys to encrypt to for an entry in `folder`: one per distinct
    /// key, in `.gpg-id` order. Refused, with every unsatisfied line named,
    /// unless every line is satisfied. Pins the `.gpg-id` the first time.
    pub fn resolve(&mut self, root: &Path, folder: &str, now: u32) -> Result<Vec<PublicKey>, WriteError> {
        let recipients = self.folder_recipients(root, folder, now)?;
        if recipients.changed {
            return Err(WriteError::RecipientsChanged { gpg_id: recipients.gpg_id });
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
            }
        }
        if recipients.lines.is_empty() {
            missing.push(format!("{} names no key", recipients.gpg_id));
        }
        if !missing.is_empty() {
            return Err(WriteError::MissingRecipients { gpg_id: recipients.gpg_id, missing });
        }
        if !self.gpg_ids.iter().any(|(path, _)| *path == recipients.gpg_id) {
            self.pin_gpg_id(root, &recipients.gpg_id)?;
        }
        Ok(keys)
    }

    fn pin_gpg_id(&mut self, root: &Path, gpg_id: &str) -> Result<(), WriteError> {
        let folder = gpg_id.strip_suffix(".gpg-id").unwrap_or_default().trim_end_matches('/');
        let (path, bytes) = governing_gpg_id(root, folder)?;
        self.gpg_ids.retain(|(old, _)| *old != path);
        self.gpg_ids.push((path, sha256_hex(&bytes)));
        self.save()
    }

    /// Accepts the current `.gpg-id` for `folder` after the person has seen
    /// that it changed.
    pub fn accept_recipients(&mut self, root: &Path, folder: &str) -> Result<(), WriteError> {
        let (path, _) = governing_gpg_id(root, folder)?;
        self.pin_gpg_id(root, &path)
    }

    /// After setup's test decrypt: binds the one line of the top-level
    /// `.gpg-id` that no pinned key satisfies to the card's key
    /// `card_fingerprint` (already pinned). Returns the lines bound: none if
    /// every line was already satisfied. If several lines are unsatisfied,
    /// paw cannot tell which is the card's, and the person has to choose
    /// ([`Self::bind`]); that is [`WriteError::MissingRecipients`].
    pub fn bind_card_after_test_decrypt(
        &mut self,
        root: &Path,
        card_fingerprint: &str,
        now: u32,
    ) -> Result<Vec<String>, WriteError> {
        let card_fingerprint = card_fingerprint.trim().to_ascii_uppercase();
        if !self.keys.iter().any(|k| k.fingerprint == card_fingerprint && k.source == KeySource::Card) {
            return Err(WriteError::NotPinned { fingerprint: card_fingerprint });
        }
        let recipients = self.folder_recipients(root, "", now)?;
        let open: Vec<String> = recipients
            .lines
            .iter()
            .filter(|line| !matches!(line.status, LineStatus::Ready { .. }))
            .map(|line| line.line.clone())
            .collect();
        match open.as_slice() {
            [] => Ok(Vec::new()),
            [line] => {
                self.bind(line, &card_fingerprint)?;
                Ok(open)
            }
            _ => Err(WriteError::MissingRecipients { gpg_id: recipients.gpg_id, missing: open }),
        }
    }
}

/// Whether `line` names the certificate `info` by itself (its fingerprint,
/// a subkey's, a long key ID or an e-mail address), as it would once
/// pinned.
pub fn certificate_named_by(info: &CertificateInfo, line: &str) -> bool {
    let Ok(choice) = &info.encryption else { return false };
    PinnedKey {
        fingerprint: choice.fingerprint.clone(),
        source: KeySource::Certificate,
        primary: Some(info.primary.clone()),
        fingerprints: info.fingerprints.clone(),
        emails: info.emails.clone(),
        expires: choice.expires,
        key: choice.key.clone(),
    }
    .named_by(line)
}

//! The enrolled key (design 5.6, rule 5): the PIN is sent only to the card
//! a store was set up with.
//!
//! Setup records the key's identity in the store's state directory once the
//! test decrypt has shown that it opens the store. A [`super::KeyPort`]
//! given that record refuses, before any VERIFY, every task on a card that
//! presents other keys. Two keys match when their decryption keys have the
//! same fingerprint, and their authentication keys too where both are
//! known. The serial is recorded and shown but not compared: a second
//! YubiKey loaded with the same keys is the same key (design 3.4), and the
//! PIN is asked for it anyway, because a held PIN belongs to one plugged-in
//! card.
//!
//! "Replace my key" is the only way to accept another card: it records the
//! plugged-in key in place of the old one.

use std::path::Path;

use crate::{CardInfo, PawError};

/// The file in the state directory. Public data only.
const FILE: &str = "enrolled-key";

/// Who a key is, as read without a PIN. Nothing in it is secret.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeyIdentity {
    /// Manufacturer and serial, e.g. "0006:12345678".
    pub ident: String,
    /// Hex fingerprint of the decryption key.
    pub decryption_fingerprint: Option<String>,
    /// Hex fingerprint of the authentication (SSH) key.
    pub authentication_fingerprint: Option<String>,
}

/// A card other than the enrolled one was plugged in. For "This is a
/// different key from the one this store was set up with".
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeyMismatch {
    /// The key the store was set up with.
    pub enrolled: KeyIdentity,
    /// The key plugged in now.
    pub presented: KeyIdentity,
}

fn same(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (None, None) => true,
        _ => false,
    }
}

impl KeyIdentity {
    pub(crate) fn of(info: &CardInfo) -> Self {
        Self {
            ident: info.ident.clone(),
            decryption_fingerprint: info.decryption_fingerprint.clone(),
            authentication_fingerprint: info.authentication_fingerprint.clone(),
        }
    }

    /// The serial, as printed on the key, from `ident`.
    pub fn serial(&self) -> String {
        self.ident.rsplit(':').next().unwrap_or_default().to_owned()
    }

    /// Whether `presented` is this key: the same decryption key, and the
    /// same authentication key where this record knows it.
    pub(crate) fn accepts(&self, presented: &KeyIdentity) -> bool {
        same(&self.decryption_fingerprint, &presented.decryption_fingerprint)
            && (self.authentication_fingerprint.is_none()
                || same(&self.authentication_fingerprint, &presented.authentication_fingerprint))
    }

    /// The mismatch if `presented` is another key.
    pub(crate) fn check(&self, presented: &KeyIdentity) -> Option<KeyMismatch> {
        (!self.accepts(presented)).then(|| KeyMismatch { enrolled: self.clone(), presented: presented.clone() })
    }

    /// How a person recognises the key: "serial 12345678, decryption key
    /// ending 2C11 41DE".
    pub fn describe(&self) -> String {
        let tail = self
            .decryption_fingerprint
            .as_deref()
            .map(|fpr| {
                let chars: Vec<char> = fpr.chars().collect();
                let end = &chars[chars.len().saturating_sub(8)..];
                let (first, last) = end.split_at(end.len().min(4));
                format!(
                    ", decryption key ending {} {}",
                    first.iter().collect::<String>(),
                    last.iter().collect::<String>()
                )
            })
            .unwrap_or_default();
        format!("serial {}{tail}", self.serial())
    }

    fn to_text(&self) -> String {
        let mut text = format!("ident={}\n", self.ident);
        if let Some(fpr) = &self.decryption_fingerprint {
            text.push_str(&format!("decryption_fingerprint={fpr}\n"));
        }
        if let Some(fpr) = &self.authentication_fingerprint {
            text.push_str(&format!("authentication_fingerprint={fpr}\n"));
        }
        text
    }

    fn from_text(text: &str) -> Option<Self> {
        let value = |name: &str| {
            text.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix('=')).map(str::to_owned)
        };
        let fingerprint = |name: &str| {
            value(name).filter(|fpr| !fpr.is_empty() && fpr.bytes().all(|b| b.is_ascii_hexdigit()))
        };
        let identity = Self {
            ident: value("ident")?,
            decryption_fingerprint: fingerprint("decryption_fingerprint"),
            authentication_fingerprint: fingerprint("authentication_fingerprint"),
        };
        // A record without a decryption key would accept any card that has
        // none; there is no such store.
        identity.decryption_fingerprint.is_some().then_some(identity)
    }
}

fn io(what: &str, e: std::io::Error) -> PawError {
    PawError::Store(format!("could not {what} the enrolled key: {e}"))
}

/// Records `identity` as the store's key, replacing any earlier one.
pub fn save_enrolment(state_dir: &Path, identity: &KeyIdentity) -> Result<(), PawError> {
    if identity.ident.contains(['\n', '\r'])
        || [&identity.decryption_fingerprint, &identity.authentication_fingerprint]
            .iter()
            .any(|fpr| fpr.as_deref().is_some_and(|f| !f.bytes().all(|b| b.is_ascii_hexdigit())))
    {
        return Err(PawError::Store("not a key identity".into()));
    }
    let temporary = state_dir.join(format!("{FILE}.tmp"));
    std::fs::write(&temporary, identity.to_text()).map_err(|e| io("save", e))?;
    std::fs::rename(&temporary, state_dir.join(FILE)).map_err(|e| io("save", e))
}

/// The key the store in `state_dir` was set up with, if setup recorded one.
/// A record that cannot be read is an error, not "none": treating it as
/// none would send the PIN to any card.
pub fn load_enrolment(state_dir: &Path) -> Result<Option<KeyIdentity>, PawError> {
    match std::fs::read_to_string(state_dir.join(FILE)) {
        Ok(text) => KeyIdentity::from_text(&text)
            .map(Some)
            .ok_or_else(|| PawError::Store("the enrolled key record is damaged".into())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io("read", e)),
    }
}

/// The key the store in `state_dir` was set up with, if any.
#[uniffi::export]
pub fn enrolled_key(state_dir: String) -> Result<Option<KeyIdentity>, PawError> {
    load_enrolment(Path::new(&state_dir))
}

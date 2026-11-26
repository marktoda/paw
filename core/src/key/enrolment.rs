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

use super::CardInfo;
use crate::PawError;
use crate::files::values::{self, Values};

/// The file in the state directory. Public data only.
const TEMP_PREFIX: &str = ".enrolled-key.tmp-";

/// The serial printed on a key, from its `ident` ("0006:12345678" is
/// "12345678"). The screens group its digits for reading.
#[uniffi::export]
pub fn key_serial(ident: String) -> String {
    ident.rsplit(':').next().unwrap_or_default().to_owned()
}

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

    /// The serial, as printed on the key; see [`key_serial`].
    pub fn serial(&self) -> String {
        key_serial(self.ident.clone())
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
            .clone()
            .map(|fpr| format!(", decryption key ending {}", crate::recipients::fingerprint_ending(fpr).join(" ")))
            .unwrap_or_default();
        format!("serial {}{tail}", self.serial())
    }

    fn pairs(&self) -> Vec<(&'static str, &str)> {
        let mut pairs = vec![("ident", self.ident.as_str())];
        if let Some(fpr) = &self.decryption_fingerprint {
            pairs.push(("decryption_fingerprint", fpr));
        }
        if let Some(fpr) = &self.authentication_fingerprint {
            pairs.push(("authentication_fingerprint", fpr));
        }
        pairs
    }

    fn from_values(values: &Values) -> Option<Self> {
        let fingerprint = |name: &str| {
            values
                .get(name)
                .filter(|fpr| !fpr.is_empty() && fpr.bytes().all(|b| b.is_ascii_hexdigit()))
                .map(str::to_owned)
        };
        let identity = Self {
            ident: values.get("ident")?.to_owned(),
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
    values::write(&state_dir.join(crate::files::layout::ENROLLED_KEY), TEMP_PREFIX, identity.pairs()).map_err(|e| io("save", e))
}

/// The key the store in `state_dir` was set up with, if setup recorded one.
/// A record that cannot be read is an error, not "none": treating it as
/// none would send the PIN to any card.
pub fn load_enrolment(state_dir: &Path) -> Result<Option<KeyIdentity>, PawError> {
    match values::read(&state_dir.join(crate::files::layout::ENROLLED_KEY)).map_err(|e| io("read", e))? {
        Some(found) => KeyIdentity::from_values(&found)
            .map(Some)
            .ok_or_else(|| PawError::Store("the enrolled key record is damaged".into())),
        None => Ok(None),
    }
}

/// How a refusal of another card reads, wherever it is reported: "a
/// different key from the one this store was set up with: ...".
pub(crate) fn mismatch_message(mismatch: &KeyMismatch) -> String {
    format!(
        "a different key from the one this store was set up with: it was set up with {}, this is {}",
        mismatch.enrolled.describe(),
        mismatch.presented.describe()
    )
}

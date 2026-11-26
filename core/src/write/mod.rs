//! Adding, editing, deleting, renaming and moving entries.
//!
//! Every change is one local commit of one entry, worded the way `pass`
//! words it, made through [`crate::sync`]'s safe commit path. None of it
//! needs the key, the PIN or the network: encrypting needs only public
//! keys, pinned on this device ([`keyring`]). The commits are exactly what
//! the "not yet pushed" queue lists and what integrate replays.
//!
//! Plaintext stays in memory that is wiped when dropped ([`EntryDoc`],
//! `Zeroizing`), is never written anywhere unencrypted, and never appears in
//! an error or a `Debug` output. What `pgp` copies internally while
//! encrypting is outside paw's reach and is not wiped.
//!
//! [`EntryDoc`]: crate::entry::EntryDoc

pub mod cert;
pub mod draft;
pub mod keyring;
pub mod ops;
pub mod scopes;

use pgp::packet::PublicKey;
use rand::rngs::OsRng;

use crate::PawError;
use crate::sync::Author;
pub use draft::{DraftField, DraftProblem, EntryDraft};
pub use keyring::{FolderRecipients, KeySource, Keyring, LineStatus, RecipientLine};
pub use ops::{Deleted, FollowUp, FollowUpFailure, Saved, WriteStore};
pub use scopes::{RecipientScope, ScopeKey};

/// Why a change was not made. In every case nothing in the store changed.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum WriteError {
    /// The form has problems; each says what to fix.
    #[error("{}", .problems.iter().map(|p| p.message.as_str()).collect::<Vec<_>>().join(" "))]
    Invalid { problems: Vec<DraftProblem> },
    /// Some recipients `.gpg-id` names have no public key on this device
    /// (or theirs expired). Saving would encrypt to fewer keys than
    /// `.gpg-id` says, so paw does not.
    #[error("paw doesn't have the key for {} (listed in {gpg_id})", .missing.join(", "))]
    MissingRecipients { gpg_id: String, missing: Vec<String> },
    /// Who can open entries here differs from what was accepted: the
    /// governing `.gpg-id` is another file (`before_gpg_id`, `None` if none
    /// governed it) or has other lines. The person has to accept it first.
    #[error("who can open these passwords changed ({gpg_id}); review it before saving here")]
    RecipientsChanged { gpg_id: String, before_gpg_id: Option<String>, before: Vec<String>, after: Vec<String> },
    /// Setup has not recorded who can open this store yet.
    #[error("paw hasn't confirmed who can open this store yet")]
    RecipientsNotConfirmed,
    /// Moving to a folder with other recipients re-encrypts the entry,
    /// which needs it open.
    #[error("open {entry} with your key to move it to a folder with different keys")]
    NeedsOpenEntry { entry: String },
    #[error("there is no entry called {entry}")]
    NoSuchEntry { entry: String },
    #[error("there's already an entry called {entry}")]
    AlreadyExists { entry: String },
    /// The entry changed (a sync) after it was opened for editing. Saving
    /// would lose that change.
    #[error("{entry} changed since you opened it; open it again to edit it")]
    ChangedSinceOpened { entry: String },
    /// A certificate could not be read or used.
    #[error("{message}")]
    Certificate { message: String },
    /// The certificate has no key that may encrypt now.
    #[error("the key {fingerprint} can't be used to encrypt: {reason}")]
    NoEncryptionKey { fingerprint: String, reason: String },
    /// A binding names a key that is not pinned.
    #[error("there is no saved key {fingerprint}")]
    NotPinned { fingerprint: String },
    /// A password policy that cannot be used.
    #[error("{message}")]
    Policy { message: String },
    /// See [`PawError::StoreBusy`].
    #[error("{}", crate::lock::STORE_BUSY)]
    StoreBusy,
    #[error("{message}")]
    Failed { message: String },
}

impl From<PawError> for WriteError {
    fn from(error: PawError) -> Self {
        match error {
            PawError::StoreBusy => Self::StoreBusy,
            other => Self::Failed { message: other.to_string() },
        }
    }
}

impl From<crate::generate::PolicyError> for WriteError {
    fn from(error: crate::generate::PolicyError) -> Self {
        Self::Policy { message: error.to_string() }
    }
}

/// Who paw's commits are by: neutral, and the `.invalid` domain can never
/// be anyone's address. The person's global git configuration is never
/// read.
pub(crate) fn author() -> Author {
    Author { name: "paw".into(), email: "paw@paw.invalid".into() }
}

/// Encrypts an entry to every key in `keys`, as `pass` does with
/// `gpg -e -r ... -r ...`: SEIPD v1 with AES-256, one recipient packet per
/// key, no compression. The plaintext is streamed from the caller's buffer
/// rather than copied into a new one.
pub(crate) fn encrypt(keys: &[PublicKey], plaintext: &[u8]) -> Result<Vec<u8>, WriteError> {
    encrypt_with(OsRng, keys, plaintext)
}

/// [`encrypt`], drawing its randomness from `rng`.
pub(crate) fn encrypt_with<R: rand::Rng + rand::CryptoRng>(
    mut rng: R,
    keys: &[PublicKey],
    plaintext: &[u8],
) -> Result<Vec<u8>, WriteError> {
    use pgp::composed::MessageBuilder;
    use pgp::crypto::sym::SymmetricKeyAlgorithm;

    if keys.is_empty() {
        return Err(WriteError::Failed { message: "there is no key to encrypt to".into() });
    }
    let failed = |e: pgp::errors::Error| WriteError::Failed { message: format!("could not encrypt: {e}") };
    let mut builder = MessageBuilder::from_reader("", plaintext).seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    for key in keys {
        builder.encrypt_to_key(&mut rng, key).map_err(failed)?;
    }
    builder.to_vec(rng).map_err(failed)
}

/// Seconds since 1970, as recipients' expiry is measured.
pub(crate) fn now_secs() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

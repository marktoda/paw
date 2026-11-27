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

mod commit;
pub mod draft;
pub mod held_back;
pub mod keyring;
pub mod ops;
pub mod scopes;

use pgp::packet::PublicKey;
use rand::rngs::OsRng;

use crate::fault::Fault;
pub use commit::Author;
pub(crate) use commit::{TreeEdit, commit_edits};
pub use draft::{DraftField, DraftProblem, EntryDraft};
pub use keyring::{FolderRecipients, Keyring, LineStatus, RecipientLine};
pub use ops::{Deleted, FollowUp, FollowUpFailure, Saved, WriteStore};
pub use scopes::{RecipientScope, ScopeKey};

/// Why a change was not made. In every case nothing in the store changed.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum WriteError {
    /// The form has problems; each says what to fix.
    #[error("{}", .problems.iter().map(|p| p.message.as_str()).collect::<Vec<_>>().join(" "))]
    Invalid { problems: Vec<DraftProblem> },
    /// Some recipients `.gpg-id` names have no public key on this device.
    /// Saving would encrypt to fewer keys than
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
    #[error("there is no entry called {entry}")]
    NoSuchEntry { entry: String },
    #[error("there's already an entry called {entry}")]
    AlreadyExists { entry: String },
    /// The entry changed (a sync) after it was opened for editing. Saving
    /// would lose that change.
    #[error("{entry} changed since you opened it; open it again to edit it")]
    ChangedSinceOpened { entry: String },
    #[error("{message}")]
    Policy { message: String },
    /// Underneath the change: the store, a refused name, or the files.
    #[error(transparent)]
    Fault { fault: Fault },
}

crate::fault::workflow_error!(WriteError);

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
pub(crate) fn encrypt(keys: &[PublicKey], plaintext: &[u8]) -> Result<Vec<u8>, Fault> {
    encrypt_with(OsRng, keys, plaintext)
}

pub(crate) fn encrypt_with<R: rand::Rng + rand::CryptoRng>(
    mut rng: R,
    keys: &[PublicKey],
    plaintext: &[u8],
) -> Result<Vec<u8>, Fault> {
    use pgp::composed::MessageBuilder;
    use pgp::crypto::sym::SymmetricKeyAlgorithm;

    if keys.is_empty() {
        return Err(Fault::local("there is no key to encrypt to"));
    }
    let failed = |e: pgp::errors::Error| Fault::local(format!("could not encrypt: {e}"));
    let mut builder = MessageBuilder::from_reader("", plaintext).seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    for key in keys {
        builder.encrypt_to_key(&mut rng, key).map_err(failed)?;
    }
    builder.to_vec(rng).map_err(failed)
}

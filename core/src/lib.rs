//! paw-core: the platform-independent part of paw.
//!
//! The private key never leaves the OpenPGP card (a YubiKey). This crate
//! parses a `pass` entry, asks the card to unwrap the session key, and does
//! the symmetric decryption itself. The app supplies only a way to exchange
//! raw APDUs with the card (see [`key::CardConnection`]).
//!
//! The same card signs the SSH login to the git host ([`ssh`]), which is how
//! the password store is cloned and pushed ([`sync`]).

pub mod clock;
pub mod copies;
pub mod entry;
pub mod fault;
pub mod ffi;
pub mod fingerprint;
pub mod path;
pub mod recipients;
pub mod remote;
pub mod report;
#[cfg(feature = "sim")]
pub mod sim;
pub mod ssh;
pub mod store;
pub mod sync;
pub mod setup;
pub mod key;
pub mod task;
pub mod generate;
pub mod files;
pub mod lock;
pub mod otp;
pub mod open;
pub mod github;
mod git_transport;
pub mod write;

uniffi::setup_scaffolding!();

/// Takes `mutex` even if a thread panicked holding it. Only for values
/// written whole (one assignment, or a file saved by rename), which a
/// panic cannot leave half written.
pub(crate) fn locked<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum PawError {
    #[error("this is not an encrypted pass entry: {0}")]
    NotAnEntry(String),
    /// A file name that could reach outside the store or into git's own
    /// files. Nothing was written.
    #[error("not a safe name for an entry: {0}")]
    UnsafePath(String),
    #[error("git error: {0}")]
    Git(String),
    /// The store on this device could not be read, or holds no such thing.
    #[error("{0}")]
    Store(String),
    /// Another part of paw, or the AutoFill extension, held the store for
    /// longer than paw waits. Nothing was changed, the store is not broken,
    /// and trying again will work.
    #[error("{}", lock::STORE_BUSY)]
    StoreBusy,
    /// The entry decrypted, but to something other than text. It is left
    /// exactly as it is.
    #[error("this entry is not text, so paw cannot show or edit it")]
    NotText,
    #[error("{0}")]
    InvalidField(String),
    /// The entry has no live Face ID copy (none was kept, it ended, or the
    /// entry changed since). Open it with the key.
    #[error("this entry needs your key")]
    CopyUnavailable,
    /// The person closed the Face ID prompt. The copy is still there.
    #[error("Face ID was cancelled")]
    FaceIdCancelled,
    #[error("Face ID didn't recognise you")]
    FaceIdNotRecognised,
    #[error("Face ID is turned off until you enter your iPhone passcode")]
    FaceIdLockedOut,
    #[error("Face ID isn't set up on this iPhone, so saved copies can't be opened")]
    FaceIdNotSetUp,
    /// The Face ID key was invalidated (enrolment changed or the passcode
    /// was removed). Every copy has been wiped.
    #[error("Face ID changed on this iPhone, so opened passwords were cleared")]
    FaceIdChanged,
    /// The copies folder or the keystore failed in some other way. Never
    /// carries entry names or contents.
    #[error("Face ID copies: {0}")]
    CopyFailed(String),
}

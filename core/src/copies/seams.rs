//! What the copies need from the phone, implemented in Swift.
//!
//! The core decides what a copy holds, when it ends and when it is wiped.
//! It cannot keep a key that only the enrolled face can use itself
//! ([`DeviceKeystore`]); its clocks are [`crate::clock::Clock`]. On Linux
//! and in the simulator, `crate::sim::copies` stands in for both.

/// Why the keystore could not do what was asked. Each variant is a
/// different screen (design.md 2.5), so they are kept apart.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum KeystoreError {
    /// The person dismissed the Face ID prompt. Nothing is wiped.
    #[error("Face ID was cancelled")]
    Cancelled,
    /// Face ID ran and did not match.
    #[error("Face ID did not recognise the face")]
    NotRecognised,
    /// Too many failed matches: biometry stays off until the passcode is
    /// entered on the lock screen.
    #[error("Face ID is locked out")]
    LockedOut,
    /// No face is enrolled, so the key cannot be used.
    #[error("Face ID is not set up")]
    NotEnrolled,
    /// The key no longer exists or can never be used again: Face ID
    /// enrolment changed (`biometryCurrentSet`) or the passcode was removed
    /// (`WhenPasscodeSetThisDeviceOnly`). Every copy sealed with it is
    /// garbage, and the core wipes them all.
    #[error("the Face ID key is gone")]
    KeyGone,
    /// Anything else. `message` must not contain secret data.
    #[error("{message}")]
    Failed { message: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for KeystoreError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Failed { message: e.reason }
    }
}

/// A key held by the device that only the enrolled face can use.
///
/// Only a fresh random 32-byte data key ever crosses this boundary to be
/// sealed: each copy's text is encrypted in Rust (see `format`), so
/// plaintext never reaches Swift to be stored.
///
/// On iOS this is `EnclaveKeystore` (app/Shared/Platform). The passcode
/// must never open a copy.
#[uniffi::export(with_foreign)]
pub trait DeviceKeystore: Send + Sync {
    /// Encrypts `secret` (a 32-byte data key) to the device key, creating
    /// the key if there is none. Never prompts.
    fn seal(&self, secret: Vec<u8>) -> Result<Vec<u8>, KeystoreError>;

    /// Decrypts what [`Self::seal`] returned. Shows Face ID with `reason`
    /// and blocks until it answers. No passcode fallback.
    fn unseal(&self, sealed: Vec<u8>, reason: String) -> Result<Vec<u8>, KeystoreError>;

    /// A 32-byte secret, usable without a prompt while the phone is
    /// unlocked, created on first use and the same in the app and the
    /// extension. It names the copy files and seals their metadata, so a
    /// reader of the directory alone learns neither which entries have
    /// copies nor until when.
    fn index_key(&self) -> Result<Vec<u8>, KeystoreError>;

    /// Destroys the seal key and the index key, so that every blob sealed
    /// so far can never be opened. The next `seal` or `index_key` makes new
    /// ones.
    fn reset(&self) -> Result<(), KeystoreError>;
}

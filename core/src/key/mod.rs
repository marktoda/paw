//! The plugged-in key: one [`KeySession`] per connected card, and what the
//! card is asked to do for a task (the tasks themselves are
//! [`crate::task`]'s).

pub(crate) mod apdu;
pub(crate) mod card;
mod decrypt;
pub mod enrolment;
pub(crate) mod kdf;
pub mod keylog;
mod port;
mod session;
mod signer;
mod task;
mod touch;
pub mod touch_clock;
mod wire;

use std::time::Duration;

pub use card::{CardConnection, CardInfo, ConnectionError, KeySlot, KeyTouch};
pub use enrolment::{KeyIdentity, KeyMismatch};
pub use kdf::PinKdfStatus;
pub use keylog::{KeyLogLine, clear_key_log, key_log, key_log_text};
pub use port::KeyPort;
pub(crate) use decrypt::Plan;
pub(crate) use session::{FirstStage, Halt, PinPrompt};
pub(crate) use signer::authenticate;
pub(crate) use task::TaskSide;
pub use session::KeySession;
pub use signer::{SessionSigner, SignFailure, SshSigner};

/// paw forgets the PIN once it has spent this long in the background
/// (owner's decision; locking the phone alone does not count).
pub const PIN_BACKGROUND_LIMIT: Duration = Duration::from_secs(15 * 60);

/// With a slot whose touch policy is CACHED (or unknown), the card may
/// answer at once from an earlier touch. `Touch` is shown only if the card
/// is still waiting this long after the command went out, so a cached touch
/// does not flash a prompt.
pub const CACHED_TOUCH_GRACE: Duration = Duration::from_millis(400);

/// A refusal quicker than this was not a touch timeout: a YubiKey waits
/// about 15 seconds for a touch before it gives up.
pub(crate) const FAST_REFUSAL: Duration = Duration::from_secs(1);

/// The PIN lengths an OpenPGP card accepts for the user PIN, in bytes.
/// Anything outside is refused before it reaches the card, so it costs no
/// try.
pub const MIN_PIN_LEN: usize = 6;
pub const MAX_PIN_LEN: usize = 127;

/// The PIN rules as the key sheet says and checks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct PinRules {
    /// [`MIN_PIN_LEN`] and [`MAX_PIN_LEN`]: the PIN's length in bytes as
    /// typed (UTF-8), which is how it goes to the core.
    pub min_length: u32,
    pub max_length: u32,
    /// [`PIN_BACKGROUND_LIMIT`], in minutes.
    pub background_limit_minutes: u32,
}

#[uniffi::export]
pub fn pin_rules() -> PinRules {
    PinRules {
        min_length: MIN_PIN_LEN as u32,
        max_length: MAX_PIN_LEN as u32,
        background_limit_minutes: (PIN_BACKGROUND_LIMIT.as_secs() / 60) as u32,
    }
}

/// Why a key task did not do what was asked. Each variant is its own
/// screen; none carries a PIN, plaintext or APDU data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum KeyError {
    #[error("wrong PIN, {tries_left} tries left")]
    WrongPin { tries_left: u8 },
    /// The PIN paw was holding no longer works (it was changed on another
    /// computer). One try was used finding out; paw has dropped it.
    #[error("the key no longer accepts the PIN paw had, {tries_left} tries left")]
    HeldPinRejected { tries_left: u8 },
    #[error("the PIN is blocked; unblock it with the admin PIN")]
    PinBlocked,
    /// Shorter than [`MIN_PIN_LEN`] or longer than [`MAX_PIN_LEN`]. It was
    /// not sent, so no try was used.
    #[error("a PIN has {} to {} characters", MIN_PIN_LEN, MAX_PIN_LEN)]
    PinFormat,
    /// paw forgot the PIN (Lock now, or [`PIN_BACKGROUND_LIMIT`] in the
    /// background) while a git login was under way, when it can no longer ask. Also why
    /// the PIN is asked again when the key turns out to have lost its
    /// verification during a decryption (or a signature fails for that):
    /// paw has then dropped the PIN, and no try was used.
    #[error("paw forgot the PIN before the key was used")]
    PinForgotten,
    /// The PIN is still held.
    #[error("the key wasn't touched in time")]
    TouchTimeout,
    /// The key was unplugged, or stopped answering, mid-operation. The PIN
    /// has been dropped.
    #[error("the key was unplugged")]
    KeyRemoved,
    /// None of the keys the entry is encrypted for is the one plugged in.
    /// Found before the PIN or a touch was asked for.
    #[error("not for this key: {recipients:?}")]
    NotForThisKey { recipients: Vec<crate::recipients::Recipient> },
    /// The key plugged in is not the one the store was set up with. Found
    /// before any PIN reached it.
    #[error("{}", enrolment::mismatch_message(.mismatch))]
    DifferentKey { mismatch: Box<KeyMismatch> },
    #[error("this key has no decryption key")]
    NoDecryptionKey,
    #[error("this key has no authentication key for SSH")]
    NoAuthenticationKey,
    /// The authentication key is `algorithm` ("RSA", "ECDSA P-256", …),
    /// and paw logs in with Ed25519 only.
    #[error("this key's authentication key is {algorithm}, not Ed25519")]
    AuthenticationKeyNotEd25519 { algorithm: String },
    /// The entry file could not be read or parsed, or did not decrypt to
    /// text. `detail` never holds entry contents.
    #[error("the entry can't be read: {detail}")]
    EntryUnreadable { detail: String },
    /// The key worked, but the entry did not open with what it produced
    /// (an entry with hidden recipients that is for another key).
    #[error("this key couldn't open the entry: {detail}")]
    DecryptFailed { detail: String },
    #[error("the key answered unexpectedly: {detail}")]
    Reader { detail: String },
    /// paw itself failed while the key was in use (a panic, a thread that
    /// could not start). Nothing is wrong with the key.
    #[error("paw hit an internal error: {detail}")]
    Internal { detail: String },
    /// See [`crate::PawError::StoreBusy`]: the entry could not be read.
    #[error("{}", crate::lock::STORE_BUSY)]
    StoreBusy,
}

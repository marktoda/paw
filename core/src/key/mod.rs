//! The plugged-in key: one [`KeySession`] per connected card, and what the
//! card is asked to do for a task (the tasks themselves are
//! [`crate::task`]'s).
//!
//! The session holds what lasts while the key stays plugged in: what the
//! card said about itself, and the PIN once it has been verified.
//!
//! - [`port`]: where keys are plugged in and out.
//! - [`session`]: one connected card; the PIN; one card job at a time.
//! - [`task`]: what the key needs of the task it works for.
//! - [`apdu`]: the card's commands and status words, defined once.
//! - [`card`]: the card as the openpgp-card library reads it.
//! - [`wire`]: the transport wrapper that sees every command go out.
//! - [`touch`]: when to show `Touch`.
//! - [`decrypt`]: an entry, split into what needs the card and what not.
//! - [`signer`]: an SSH signer that signs through the session.
//! - [`enrolment`]: the key the store was set up with; no PIN to another.
//! - [`keylog`]: what paw and the card said to each other, for diagnosis.

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

pub use card::{CardConnection, CardInfo, ConnectionError, KeyTouch};
pub use decrypt::MAX_DECIPHERS;
pub use enrolment::{KeyIdentity, KeyMismatch};
pub use kdf::PinKdfStatus;
pub use keylog::{KeyLogLine, clear_key_log, key_log, key_log_text};
pub use port::KeyPort;
pub(crate) use decrypt::Plan;
pub(crate) use session::{Halt, PinPrompt};
pub(crate) use signer::authenticate;
pub(crate) use task::TaskSide;
pub use session::KeySession;
pub use signer::SessionSigner;

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

/// The PIN lengths an OpenPGP card accepts for the user PIN. Anything
/// outside is refused before it reaches the card, so it costs no try.
pub const MIN_PIN_LEN: usize = 6;
pub const MAX_PIN_LEN: usize = 127;

/// Why a key task did not do what was asked. Each variant is its own
/// screen; none carries a PIN, plaintext or APDU data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum KeyError {
    /// The PIN typed was wrong.
    #[error("wrong PIN, {tries_left} tries left")]
    WrongPin { tries_left: u8 },
    /// The PIN paw was holding no longer works (it was changed on another
    /// computer). One try was used finding out; paw has dropped it.
    #[error("the key no longer accepts the PIN paw had, {tries_left} tries left")]
    HeldPinRejected { tries_left: u8 },
    #[error("the PIN is blocked; unblock it with the admin PIN")]
    PinBlocked,
    /// Not 6 to 127 characters. It was not sent, so no try was used.
    #[error("a PIN has 6 to 127 characters")]
    PinFormat,
    /// paw forgot the PIN (Lock now, or 15 minutes in the background)
    /// while a git login was under way, when it can no longer ask. Also why
    /// the PIN is asked again when the key turns out to have lost its
    /// verification during a decryption (or a signature fails for that):
    /// paw has then dropped the PIN, and no try was used.
    #[error("paw forgot the PIN before the key was used")]
    PinForgotten,
    /// Nobody touched the key in time. The PIN is still held.
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
    /// The reader or the card answered something unexpected.
    #[error("the key answered unexpectedly: {detail}")]
    Reader { detail: String },
    /// paw itself failed while the key was in use (a panic, a thread that
    /// could not start). Nothing is wrong with the key.
    #[error("paw hit an internal error: {detail}")]
    Internal { detail: String },
    /// See [`crate::PawError::StoreBusy`]: the entry could not be read.
    #[error("{}", crate::lock::STORE_BUSY)]
    StoreBusy,
    #[error("cancelled")]
    Cancelled,
}

/// How far the work after the key has got, when it is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct TaskProgress {
    pub done: u32,
    pub total: u32,
    /// Bytes received so far, for a download.
    pub bytes: Option<u64>,
}

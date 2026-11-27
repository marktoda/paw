//! Key tasks as the app sees them: one [`KeyTask`] per thing the person
//! asked for that needs the key (open an entry, sign a git login, check
//! the key during setup, re-encrypt), walking through the key sheet's
//! stages (design.md 2.3). The sheet only renders [`KeyTaskState`]: it
//! never combines states or decides anything.
//!
//! This module knows every workflow that runs as a key task: what each is
//! for, what it produces and how the work after the key fails. The key
//! ([`crate::key`]) knows none of them.

mod run;
mod start;
pub(crate) mod workflows;

use std::sync::Arc;

pub(crate) use start::{After, DecryptWork, Work};

use crate::PawError;
use crate::entry::opened::OpenedEntry;
use crate::key::{Halt, KeyError};
use crate::setup::SetupError;
use crate::sync::{SyncError, TransferProgress};
use crate::sync::operation::SyncReport;
use crate::write::held_back::ReencryptReport;

pub use run::KeyTask;

/// What a task is for: the key log names it, and setup's key check says
/// which entry it opens.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum KeyPurpose {
    OpenEntry { entry: String },
    /// Opens one entry to prove the key works, and keeps nothing. `None`
    /// when setup found no entry to open or could not open the store.
    CheckKey { entry: Option<String> },
    SignIn { login: LoginPurpose },
    /// Key diagnostics: one signature nobody is meant to touch for, to see
    /// what the key answers when a touch times out.
    CheckTouchTimeout,
    /// Opens held-back changes, one touch each, to encrypt them again for
    /// their folder's new list.
    Reencrypt,
}

/// Why a git login is being signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LoginPurpose {
    Clone,
    Fetch,
    Push,
}

#[derive(Clone, uniffi::Enum)]
pub enum KeyOutcome {
    /// The entry is open. `face_id_until_ms` is when its Face ID copy ends
    /// (wall clock), or `None` if none was kept; `copy_failed` says why a
    /// copy that was asked for was not kept.
    Opened { entry: Arc<OpenedEntry>, face_id_until_ms: Option<i64>, copy_failed: Option<PawError> },
    /// The key opened the test entry; its text was wiped at once.
    KeyChecked,
    /// The login was signed and the caller has finished with the key. For
    /// `CheckTouchTimeout`: the key signed, so it was touched, or its
    /// authentication slot asks for no touch.
    SignedIn,
    /// A send or a check went through.
    Synced { report: SyncReport },
    /// The re-encrypt ran to the end: what it did and what it left.
    Reencrypted { report: ReencryptReport },
}

impl std::fmt::Debug for KeyOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Opened { face_id_until_ms, copy_failed, .. } => f
                .debug_struct("Opened")
                .field("entry", &"<redacted>")
                .field("face_id_until_ms", face_id_until_ms)
                .field("copy_failed", copy_failed)
                .finish(),
            Self::KeyChecked => f.write_str("KeyChecked"),
            Self::SignedIn => f.write_str("SignedIn"),
            Self::Synced { report } => f.debug_struct("Synced").field("report", report).finish(),
            Self::Reencrypted { report } => f.debug_struct("Reencrypted").field("report", report).finish(),
        }
    }
}

/// How a task failed: the key, or the work after it.
#[derive(Debug, thiserror::Error)]
pub enum TaskFailure {
    #[error(transparent)]
    Key(KeyError),
    #[error(transparent)]
    Sync(SyncError),
    #[error(transparent)]
    Setup(SetupError),
    /// The task was cancelled: it has already ended `Cancelled`.
    #[error("cancelled")]
    Cancelled,
}

impl From<KeyError> for TaskFailure {
    fn from(error: KeyError) -> Self {
        Self::Key(error)
    }
}

impl From<Halt> for TaskFailure {
    fn from(halt: Halt) -> Self {
        match halt {
            Halt::Fail(error) => Self::Key(error),
            Halt::Cancelled => Self::Cancelled,
        }
    }
}

impl From<SyncError> for TaskFailure {
    fn from(error: SyncError) -> Self {
        Self::Sync(error)
    }
}

impl From<SetupError> for TaskFailure {
    fn from(error: SetupError) -> Self {
        Self::Setup(error)
    }
}

/// How far the work after the key has got, when it is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct TaskProgress {
    pub done: u32,
    pub total: u32,
    /// Bytes received so far, for a download.
    pub bytes: Option<u64>,
}

impl From<TransferProgress> for TaskProgress {
    fn from(update: TransferProgress) -> Self {
        Self { done: update.received_objects, total: update.total_objects, bytes: Some(update.received_bytes) }
    }
}

/// The key sheet's stage (design.md 2.3).
#[derive(Debug, Clone, uniffi::Enum)]
pub enum KeyTaskState {
    /// No key is connected. The task goes on by itself once one is.
    PlugIn,
    /// Ask for the PIN. `tries_left` is what the key says remain.
    /// `forgot_after_background`: paw forgot the PIN after
    /// [`crate::key::PIN_BACKGROUND_LIMIT`] away. `last_error` is why the
    /// PIN is asked again (a wrong PIN, one of a length no card takes, or a
    /// held PIN the key no longer accepts).
    Pin { tries_left: u8, forgot_after_background: bool, last_error: Option<KeyError> },
    /// The key is finishing something else (an earlier command, perhaps a
    /// cancelled one still waiting for its touch). This task is queued.
    Busy,
    /// The key is waiting for a touch for this task's command.
    Touch,
    /// The key or the work after it is busy for this task.
    Working { progress: Option<TaskProgress> },
    Done { outcome: KeyOutcome },
    /// The key failed.
    Failed { error: KeyError },
    /// The key did its part and the sync after it failed.
    SyncFailed { error: SyncError },
    /// The key did its part and the setup step after it failed.
    SetupFailed { error: SetupError },
    Cancelled,
}

#[uniffi::export]
impl KeyTaskState {
    /// Done, failed or cancelled: nothing changes it any more.
    pub fn is_final(&self) -> bool {
        match self {
            Self::Done { .. } | Self::Failed { .. } | Self::SyncFailed { .. } | Self::SetupFailed { .. } | Self::Cancelled => {
                true
            }
            Self::PlugIn | Self::Pin { .. } | Self::Busy | Self::Touch | Self::Working { .. } => false,
        }
    }
}

impl KeyTaskState {
    fn ended(result: Result<KeyOutcome, TaskFailure>) -> Self {
        match result {
            Ok(outcome) => Self::Done { outcome },
            Err(TaskFailure::Key(error)) => Self::Failed { error },
            Err(TaskFailure::Sync(error)) => Self::SyncFailed { error },
            Err(TaskFailure::Setup(error)) => Self::SetupFailed { error },
            Err(TaskFailure::Cancelled) => Self::Cancelled,
        }
    }

    /// Whether `self` would show the person nothing new after `old`.
    fn same_as(&self, old: &Self) -> bool {
        use KeyTaskState as S;
        match (old, self) {
            (S::PlugIn, S::PlugIn) | (S::Busy, S::Busy) | (S::Touch, S::Touch) => true,
            (S::Working { progress: a }, S::Working { progress: b }) => a == b,
            (
                S::Pin { tries_left: a, forgot_after_background: b, last_error: c },
                S::Pin { tries_left: x, forgot_after_background: y, last_error: z },
            ) => a == x && b == y && c == z,
            _ => false,
        }
    }
}

/// Told every time a task's stage changes, in order, on whichever thread
/// made the change. It must return quickly and must not block; it may call
/// back into the task (`supply_pin`, `cancel`, `state`).
#[uniffi::export(with_foreign)]
pub trait KeyTaskListener: Send + Sync {
    fn state_changed(&self, state: KeyTaskState);
}

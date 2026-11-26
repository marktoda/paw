//! Key tasks as the app sees them: the key's task ([`crate::key::Running`])
//! for every workflow that runs one, with what each produces and how the
//! work after the key fails. A new keyed workflow adds its outcome and its
//! failure here; the key itself does not change.

use std::sync::Arc;
use std::time::Duration;

use crate::PawError;
use crate::copies::CopyCache;
use crate::entry::opened::OpenedEntry;
use crate::key::{
    Failure, KeyDone, KeyError, KeyPort, KeyPurpose, Running, Stage, StageListener, TaskProgress, Workflow,
};
use crate::setup::SetupError;
use crate::store::PassStore;
use crate::sync::SyncError;
use crate::sync::operation::SyncReport;
use crate::write::held_back::ReencryptReport;

/// What a finished task produced.
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

impl From<KeyDone> for KeyOutcome {
    fn from(done: KeyDone) -> Self {
        match done {
            KeyDone::Opened { entry, face_id_until_ms, copy_failed } => {
                Self::Opened { entry, face_id_until_ms, copy_failed }
            }
            KeyDone::KeyChecked => Self::KeyChecked,
            KeyDone::SignedIn => Self::SignedIn,
        }
    }
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

/// How the work after the key failed.
#[derive(Debug, Clone)]
pub enum WorkFailure {
    Sync(SyncError),
    Setup(SetupError),
}

/// Every workflow the app runs as a key task.
pub struct App;

impl Workflow for App {
    type Outcome = KeyOutcome;
    type Failure = WorkFailure;

    fn failure_name(failure: &WorkFailure) -> &'static str {
        match failure {
            WorkFailure::Sync(_) => "SyncFailed",
            WorkFailure::Setup(_) => "SetupFailed",
        }
    }
}

/// How an app task's work failed.
pub type TaskFailure = Failure<WorkFailure>;

impl From<SyncError> for Failure<WorkFailure> {
    fn from(error: SyncError) -> Self {
        Self::Work(WorkFailure::Sync(error))
    }
}

impl From<Failure<SetupError>> for Failure<WorkFailure> {
    fn from(failure: Failure<SetupError>) -> Self {
        match failure {
            Failure::Key(error) => Self::Key(error),
            Failure::Work(error) => error.into(),
        }
    }
}

impl From<SetupError> for Failure<WorkFailure> {
    fn from(error: SetupError) -> Self {
        Self::Work(WorkFailure::Setup(error))
    }
}

/// The key sheet's stage (design.md 2.3).
#[derive(Debug, Clone, uniffi::Enum)]
pub enum KeyTaskState {
    /// No key is connected. The task goes on by itself once one is.
    PlugIn,
    /// Ask for the PIN. `tries_left` is what the key says remain.
    /// `forgot_after_background` adds "paw forgot your PIN after 15
    /// minutes away." `last_error` is why the PIN is asked again (a wrong
    /// PIN, a PIN that was not 6 to 127 characters, or a held PIN the key
    /// no longer accepts).
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

impl KeyTaskState {
    /// Done, failed or cancelled: nothing changes it any more.
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            Self::Done { .. } | Self::Failed { .. } | Self::SyncFailed { .. } | Self::SetupFailed { .. } | Self::Cancelled
        )
    }
}

impl From<Stage<App>> for KeyTaskState {
    fn from(stage: Stage<App>) -> Self {
        match stage {
            Stage::PlugIn => Self::PlugIn,
            Stage::Pin { tries_left, forgot_after_background, last_error } => {
                Self::Pin { tries_left, forgot_after_background, last_error }
            }
            Stage::Busy => Self::Busy,
            Stage::Touch => Self::Touch,
            Stage::Working { progress } => Self::Working { progress },
            Stage::Done(outcome) => Self::Done { outcome },
            Stage::Failed(error) => Self::Failed { error },
            Stage::WorkFailed(WorkFailure::Sync(error)) => Self::SyncFailed { error },
            Stage::WorkFailed(WorkFailure::Setup(error)) => Self::SetupFailed { error },
            Stage::Cancelled => Self::Cancelled,
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

struct Relay(Arc<dyn KeyTaskListener>);

impl StageListener<App> for Relay {
    fn changed(&self, stage: Stage<App>) {
        self.0.state_changed(stage.into());
    }
}

/// The key's listener for `listener`.
pub(crate) fn relay(listener: Arc<dyn KeyTaskListener>) -> Arc<dyn StageListener<App>> {
    Arc::new(Relay(listener))
}

/// One operation that needs the key, as the key sheet binds to it.
#[derive(uniffi::Object)]
pub struct KeyTask(Running<App>);

impl KeyTask {
    pub(crate) fn of(task: Running<App>) -> Arc<Self> {
        Arc::new(Self(task))
    }

    /// A task that has already failed with `failure`.
    pub(crate) fn failed(purpose: KeyPurpose, listener: Arc<dyn KeyTaskListener>, failure: TaskFailure) -> Arc<Self> {
        Self::of(Running::failed(purpose, relay(listener), failure))
    }

    /// Waits for the task's thread to end. `true` if it panicked (it then
    /// failed with an internal error rather than hang).
    pub fn join(&self) -> bool {
        self.0.join()
    }
}

#[uniffi::export]
impl KeyTask {
    pub fn purpose(&self) -> KeyPurpose {
        self.0.purpose()
    }

    /// The stage to show now.
    pub fn state(&self) -> KeyTaskState {
        self.0.stage().into()
    }

    /// The PIN as typed, as bytes. Ignored (and wiped) unless the task is
    /// at the PIN stage. Clear the field after calling.
    pub fn supply_pin(&self, pin: Vec<u8>) {
        self.0.supply_pin(pin);
    }

    /// Stops the task now, at any stage. See [`crate::key`]'s task module.
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

#[uniffi::export]
impl KeyPort {
    /// Opens `entry` with the key and keeps a Face ID copy of it for
    /// `ttl_secs` in `copies` (none if `copies` is absent or `ttl_secs` is
    /// zero; at most 12 hours).
    ///
    /// An entry the key cannot open is refused before the PIN or a touch.
    pub fn open_entry(
        self: Arc<Self>,
        store: Arc<PassStore>,
        entry: String,
        copies: Option<Arc<CopyCache>>,
        ttl_secs: u32,
        listener: Arc<dyn KeyTaskListener>,
    ) -> Arc<KeyTask> {
        let copies = copies.map(|copies| (copies, Duration::from_secs(ttl_secs.into())));
        KeyTask::of(self.open_entry_task(store, entry, copies, relay(listener)))
    }

    /// Key diagnostics' touch timeout check; see
    /// [`KeyPort::touch_timeout_task`]. `Done` with `SignedIn` means the
    /// key signed: it was touched, or the slot asks for no touch.
    pub fn check_touch_timeout(self: Arc<Self>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        KeyTask::of(self.touch_timeout_task(relay(listener)))
    }
}

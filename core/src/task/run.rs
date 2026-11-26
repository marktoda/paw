//! One user-visible operation that needs the key, as the key sheet sees it.
//!
//! Each task runs on its own thread ("paw-key-task"), which waits for the
//! key to be plugged in, for the PIN and for the card; none of those waits
//! holds the card. The sheet drives it with [`KeyTask::supply_pin`] and
//! [`KeyTask::cancel`] and renders [`KeyTask::state`], which a listener is
//! also told about on every change, in order.
//!
//! Cancel is honoured at every stage and takes effect at once: the state
//! becomes `Cancelled` before `cancel` returns, and nothing changes it
//! after. A card command already waiting for a touch cannot be interrupted
//! from here; the task's thread waits for the card's answer, throws it
//! away (wiped on drop), keeps no copy, and only then lets the next job have
//! the card. The one exception is the last few milliseconds of an open,
//! after the card is done and while the entry is being handed over: a
//! cancel then is too late and the task finishes.

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use zeroize::Zeroizing;

use super::{KeyOutcome, KeyPurpose, KeyTaskListener, KeyTaskState, LoginPurpose, TaskFailure, TaskProgress};
use crate::key::keylog::error_name;
use crate::key::{Halt, KeyError, KeySession, PinPrompt, TaskSide};

struct Inner {
    state: KeyTaskState,
    /// Stages not yet given to the listener.
    queue: VecDeque<KeyTaskState>,
    /// Some thread is giving `queue` to the listener.
    draining: bool,
    /// A PIN supplied at the PIN stage, not yet taken.
    supplied: Option<Zeroizing<Vec<u8>>>,
    /// Past the point where cancel can still stop it.
    committing: bool,
    session: Option<Arc<KeySession>>,
}

/// Numbers tasks in the key log, per process.
static NEXT_TASK: AtomicU64 = AtomicU64::new(1);

pub(crate) struct TaskCore {
    id: u64,
    /// Times the card was found to have lost the PIN's verification during
    /// this task's key operation (see `KeySession::refused`).
    pin_losses: AtomicU32,
    purpose: KeyPurpose,
    listener: Arc<dyn KeyTaskListener>,
    cancelled: AtomicBool,
    panicked: AtomicBool,
    inner: Mutex<Inner>,
    changed: Condvar,
}

/// A task's purpose in one word, for the key log. Never the entry's name.
pub(super) fn purpose_words(purpose: &KeyPurpose) -> &'static str {
    match purpose {
        KeyPurpose::OpenEntry { .. } => "open",
        KeyPurpose::CheckKey { .. } => "setup check",
        KeyPurpose::SignIn { login: LoginPurpose::Clone } => "sign in to download",
        KeyPurpose::SignIn { login: LoginPurpose::Fetch } => "sign in to check for changes",
        KeyPurpose::SignIn { login: LoginPurpose::Push { .. } } => "sign in to send",
        KeyPurpose::CheckTouchTimeout => "touch timeout check",
        KeyPurpose::Reencrypt { .. } => "re-encrypt",
    }
}

/// A stage as the key log shows it. `None` for progress updates, which
/// would fill the log during a download.
pub(super) fn stage_words(state: &KeyTaskState) -> Option<String> {
    Some(match state {
        KeyTaskState::PlugIn => "PlugIn".into(),
        KeyTaskState::Pin { tries_left, forgot_after_background, last_error } => {
            let mut words = "Pin".to_string();
            if let Some(error) = last_error {
                words += &format!(" (again: {})", error_name(error));
            }
            if *tries_left < 3 {
                words += &format!(" ({tries_left} tries left)");
            }
            if *forgot_after_background {
                words += " (forgot after background)";
            }
            words
        }
        KeyTaskState::Busy => "Busy".into(),
        KeyTaskState::Touch => "Touch".into(),
        KeyTaskState::Working { progress: None } => "Working".into(),
        KeyTaskState::Working { progress: Some(_) } => return None,
        KeyTaskState::Done { .. } => "Done".into(),
        KeyTaskState::Failed { error } => format!("Failed({})", error_name(error)),
        KeyTaskState::SyncFailed { .. } => "SyncFailed".into(),
        KeyTaskState::SetupFailed { .. } => "SetupFailed".into(),
        KeyTaskState::Cancelled => "Cancelled".into(),
    })
}

impl TaskCore {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A listener that panicked did so outside this lock.
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Writes a stage change to the key log of the session the task is on.
    /// Called with the task's lock held; the log's own locks are taken
    /// under it and never the other way round.
    fn log_stage(&self, session: Option<Arc<KeySession>>, state: &KeyTaskState) {
        if let (Some(session), Some(words)) = (session, stage_words(state)) {
            let log = session.log();
            log.line(format!("{} stage {words}", self.tag()));
            if matches!(state, KeyTaskState::Pin { .. }) {
                log.pin_asked();
            } else if state.is_final() {
                log.task_ended();
            }
        }
    }

    /// `again` delivers a stage even if it looks like the current one: a
    /// PIN asked again after a refusal is news, even with the same counter.
    fn apply(inner: &mut Inner, new: KeyTaskState, again: bool) -> bool {
        if inner.state.is_final() || (!again && new.same_as(&inner.state)) {
            return false;
        }
        if !matches!(new, KeyTaskState::Pin { .. }) {
            inner.supplied = None;
        }
        inner.state = new.clone();
        inner.queue.push_back(new);
        true
    }

    pub(super) fn set(&self, new: KeyTaskState) -> bool {
        self.set_again(new, false)
    }

    fn set_again(&self, new: KeyTaskState, again: bool) -> bool {
        let applied = {
            let mut inner = self.lock();
            let applied = Self::apply(&mut inner, new.clone(), again);
            if applied {
                // Under the lock: whoever sees the stage finds it logged.
                self.log_stage(inner.session.clone(), &new);
            }
            applied
        };
        if applied {
            self.changed.notify_all();
            self.drain();
        }
        applied
    }

    /// Gives queued states to the listener, in order, without holding the
    /// lock. Whoever finds nobody else doing it does it.
    fn drain(&self) {
        {
            let mut inner = self.lock();
            if inner.draining {
                return;
            }
            inner.draining = true;
        }
        loop {
            let next = {
                let mut inner = self.lock();
                match inner.queue.pop_front() {
                    Some(state) => state,
                    None => {
                        inner.draining = false;
                        return;
                    }
                }
            };
            self.listener.state_changed(next);
        }
    }

    fn cancel(&self) {
        let session = {
            let mut inner = self.lock();
            if inner.committing || inner.state.is_final() {
                return;
            }
            self.cancelled.store(true, Ordering::SeqCst);
            Self::apply(&mut inner, KeyTaskState::Cancelled, false);
            self.log_stage(inner.session.clone(), &KeyTaskState::Cancelled);
            inner.session.clone()
        };
        self.changed.notify_all();
        self.drain();
        if let Some(session) = session {
            session.wake_card_waiters();
        }
    }

    pub(super) fn bind(self: &Arc<Self>, session: &Arc<KeySession>) {
        self.lock().session = Some(session.clone());
        let side: Arc<dyn TaskSide> = self.clone();
        session.watch(&side);
    }
}

impl TaskSide for TaskCore {
    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn tag(&self) -> String {
        format!("#{}", self.id)
    }

    fn note_pin_lost(&self) -> u32 {
        self.pin_losses.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn wake(&self) {
        let _inner = self.lock();
        self.changed.notify_all();
    }

    fn show_plug_in(&self) {
        self.set(KeyTaskState::PlugIn);
    }

    fn show_busy(&self) {
        self.set(KeyTaskState::Busy);
    }

    fn show_touch(&self) {
        self.set(KeyTaskState::Touch);
    }

    fn show_working(&self) {
        self.set(KeyTaskState::Working { progress: None });
    }

    fn wait_until(&self, done: &mut dyn FnMut() -> bool) -> Result<(), Halt> {
        let mut inner = self.lock();
        loop {
            if self.cancelled() {
                return Err(Halt::Fail(KeyError::Cancelled));
            }
            if done() {
                return Ok(());
            }
            inner = self.changed.wait(inner).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn ask_pin(
        &self,
        session: &KeySession,
        prompt: PinPrompt,
        last_error: Option<KeyError>,
    ) -> Result<Zeroizing<Vec<u8>>, Halt> {
        let again = last_error.is_some();
        let stage = KeyTaskState::Pin {
            tries_left: prompt.tries_left,
            forgot_after_background: prompt.forgot_after_background,
            last_error,
        };
        self.set_again(stage, again);
        let mut inner = self.lock();
        loop {
            if self.cancelled() {
                return Err(Halt::Fail(KeyError::Cancelled));
            }
            if session.ended() {
                return Err(Halt::Fail(KeyError::KeyRemoved));
            }
            if let Some(pin) = inner.supplied.take() {
                return Ok(pin);
            }
            inner = self.changed.wait(inner).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

/// What the work after the key sees of the task it runs in. A cancel
/// ends the task at once, whatever the work returns after it; the work
/// asks only to stop early (between network steps, before the next
/// change) and, before a change that must not happen once cancelled, to
/// [`Self::commit`].
pub(crate) struct Job<'a>(&'a TaskCore);

impl Job<'_> {
    pub(super) fn of(core: &TaskCore) -> Job<'_> {
        Job(core)
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.0.cancelled()
    }

    /// Shows `Working` with `progress`. `false` once the task is cancelled
    /// or has ended: the work should stop.
    pub(crate) fn progress(&self, progress: TaskProgress) -> bool {
        let core = self.0;
        if core.cancelled() || core.lock().state.is_final() {
            return false;
        }
        core.set(KeyTaskState::Working { progress: Some(progress) });
        !core.cancelled()
    }

    /// From here on, cancel is too late and the task ends as the work
    /// says. Fails with `Cancelled` if the cancel came first.
    pub(crate) fn commit(&self) -> Result<(), TaskFailure> {
        let core = self.0;
        let mut inner = core.lock();
        if core.cancelled() || inner.state.is_final() {
            return Err(KeyError::Cancelled.into());
        }
        inner.committing = true;
        Ok(())
    }
}

pub(super) type Body = Box<dyn FnOnce(&Arc<TaskCore>) -> Result<KeyOutcome, TaskFailure> + Send>;

/// One operation that needs the key, running on its own thread, as the
/// key sheet binds to it. See the module comment.
#[derive(uniffi::Object)]
pub struct KeyTask {
    core: Arc<TaskCore>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl KeyTask {
    pub(super) fn start(
        purpose: KeyPurpose,
        listener: Arc<dyn KeyTaskListener>,
        initial: KeyTaskState,
        body: Body,
    ) -> Arc<Self> {
        let finished = initial.is_final();
        let core = Arc::new(TaskCore {
            id: NEXT_TASK.fetch_add(1, Ordering::Relaxed),
            pin_losses: AtomicU32::new(0),
            purpose,
            listener,
            cancelled: AtomicBool::new(false),
            panicked: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                state: initial,
                queue: VecDeque::new(),
                draining: false,
                supplied: None,
                committing: false,
                session: None,
            }),
            changed: Condvar::new(),
        });
        let task = Arc::new(Self { core: core.clone(), thread: Mutex::new(None) });
        if finished {
            return task;
        }
        let spawned = std::thread::Builder::new().name("paw-key-task".into()).spawn(move || {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| body(&core)));
            let end = match result {
                Ok(result) => KeyTaskState::ended(result),
                Err(_) => {
                    core.panicked.store(true, Ordering::SeqCst);
                    KeyTaskState::Failed { error: KeyError::Internal { detail: "paw hit an internal error".into() } }
                }
            };
            core.set(end);
        });
        match spawned {
            Ok(handle) => *task.thread.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle),
            Err(e) => {
                task.core.set(KeyTaskState::Failed { error: KeyError::Internal { detail: e.to_string() } });
            }
        }
        task
    }

    pub(crate) fn failed(purpose: KeyPurpose, listener: Arc<dyn KeyTaskListener>, failure: TaskFailure) -> Arc<Self> {
        Self::start(
            purpose,
            listener,
            KeyTaskState::ended(Err(failure)),
            Box::new(|_| unreachable!("a failed task never runs")),
        )
    }

    pub(super) fn tag(&self) -> String {
        self.core.tag()
    }

    /// Waits for the task's thread to end. `true` if it panicked (it then
    /// failed with an internal error rather than hang).
    pub fn join(&self) -> bool {
        let handle = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        self.core.panicked.load(Ordering::SeqCst)
    }
}

#[uniffi::export]
impl KeyTask {
    pub fn purpose(&self) -> KeyPurpose {
        self.core.purpose.clone()
    }

    /// The stage to show now.
    pub fn state(&self) -> KeyTaskState {
        self.core.lock().state.clone()
    }

    /// The PIN as typed, as bytes. Ignored (and wiped) unless the task is
    /// at the PIN stage. Clear the field after calling.
    pub fn supply_pin(&self, pin: Vec<u8>) {
        let pin = Zeroizing::new(pin);
        let mut inner = self.core.lock();
        if matches!(inner.state, KeyTaskState::Pin { .. }) {
            inner.supplied = Some(pin);
            self.core.changed.notify_all();
        }
    }

    /// Stops the task now, at any stage. See the module comment.
    pub fn cancel(&self) {
        self.core.cancel();
    }
}

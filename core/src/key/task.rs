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

use super::session::{Halt, KeySession, Pin};
use super::{KeyError, KeyPurpose, TaskProgress};
use crate::PawError;
use crate::entry::opened::OpenedEntry;

/// What the work of one kind of task ends with: the workflows a task can
/// run, each with what it produces and how it fails after the key did its
/// part. The key's own tasks end with a [`KeyDone`].
pub trait Workflow: 'static {
    type Outcome: From<KeyDone> + Clone + Send + Sync;
    type Failure: Clone + Send + Sync;

    /// The failure's name in the key log (never its text).
    fn failure_name(failure: &Self::Failure) -> &'static str;
}

/// What a task the key runs by itself produced.
pub enum KeyDone {
    /// The entry is open. `face_id_until_ms` is when its Face ID copy ends
    /// (wall clock), or `None` if none was kept; `copy_failed` says why a
    /// copy that was asked for was not kept.
    Opened { entry: Arc<OpenedEntry>, face_id_until_ms: Option<i64>, copy_failed: Option<PawError> },
    /// The key opened the test entry; its text was wiped at once.
    KeyChecked,
    /// The login was signed, or the touch check's signature was made.
    SignedIn,
}

/// How a task's work failed: the key, or the work after it.
#[derive(Debug)]
pub enum Failure<F> {
    Key(KeyError),
    Work(F),
}

impl<F> From<KeyError> for Failure<F> {
    fn from(error: KeyError) -> Self {
        Self::Key(error)
    }
}

impl<F> From<Halt> for Failure<F> {
    fn from(halt: Halt) -> Self {
        Self::Key(halt.into())
    }
}

impl From<Halt> for KeyError {
    fn from(halt: Halt) -> Self {
        match halt {
            Halt::Fail(error) => error,
            Halt::AskPin(_) => KeyError::Internal { detail: "the PIN was needed where it cannot be asked".into() },
        }
    }
}

/// A task's stage (design.md 2.3), for a workflow `W`.
pub enum Stage<W: Workflow> {
    PlugIn,
    Pin { tries_left: u8, forgot_after_background: bool, last_error: Option<KeyError> },
    Busy,
    Touch,
    Working { progress: Option<TaskProgress> },
    Done(W::Outcome),
    /// The key failed.
    Failed(KeyError),
    /// The key did its part and the work after it failed.
    WorkFailed(W::Failure),
    Cancelled,
}

impl<W: Workflow> Clone for Stage<W> {
    fn clone(&self) -> Self {
        match self {
            Self::PlugIn => Self::PlugIn,
            Self::Pin { tries_left, forgot_after_background, last_error } => Self::Pin {
                tries_left: *tries_left,
                forgot_after_background: *forgot_after_background,
                last_error: last_error.clone(),
            },
            Self::Busy => Self::Busy,
            Self::Touch => Self::Touch,
            Self::Working { progress } => Self::Working { progress: *progress },
            Self::Done(outcome) => Self::Done(outcome.clone()),
            Self::Failed(error) => Self::Failed(error.clone()),
            Self::WorkFailed(failure) => Self::WorkFailed(failure.clone()),
            Self::Cancelled => Self::Cancelled,
        }
    }
}

impl<W: Workflow> Stage<W> {
    /// Done, failed or cancelled: nothing changes it any more.
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Done(_) | Self::Failed(_) | Self::WorkFailed(_) | Self::Cancelled)
    }

    fn ended(result: Result<W::Outcome, Failure<W::Failure>>) -> Self {
        match result {
            Ok(outcome) => Self::Done(outcome),
            Err(Failure::Key(error)) => Self::Failed(error),
            Err(Failure::Work(failure)) => Self::WorkFailed(failure),
        }
    }
}

/// Told every time a task's stage changes, in order, on whichever thread
/// made the change. It must return quickly and must not block; it may call
/// back into the task.
pub trait StageListener<W: Workflow>: Send + Sync {
    fn changed(&self, stage: Stage<W>);
}

/// A running task as the key's own code sees it, whatever its workflow:
/// the stages the card and the work after it move it through.
pub(crate) trait TaskSide: Send + Sync {
    fn cancelled(&self) -> bool;
    /// "#3": the task in the key log.
    fn tag(&self) -> String;
    /// Counts one more loss of the PIN's verification; returns how many
    /// this task has seen.
    fn note_pin_lost(&self) -> u32;
    fn wake(&self);
    fn show_busy(&self);
    fn show_touch(&self);
    fn show_working(&self);
    /// The work after the key has got this far: shown as `Working` with
    /// the count done and expected. Nothing once the task has been
    /// cancelled or has finished. `false` if it has: the work should stop.
    fn report_progress(&self, done: u32, total: u32) -> bool;
    /// From here on, cancel is too late. `false` if it already came.
    fn begin_commit(&self) -> bool;
}

struct Inner<W: Workflow> {
    state: Stage<W>,
    /// Stages not yet given to the listener.
    queue: VecDeque<Stage<W>>,
    /// Some thread is giving `queue` to the listener.
    draining: bool,
    /// A PIN supplied at the PIN stage, not yet taken.
    supplied: Option<Zeroizing<Vec<u8>>>,
    /// Past the point where cancel can still stop it.
    committing: bool,
    /// The session the task runs on, once it has one.
    session: Option<Arc<KeySession>>,
}

/// Numbers tasks in the key log, per process.
static NEXT_TASK: AtomicU64 = AtomicU64::new(1);

pub(crate) struct TaskCore<W: Workflow> {
    /// The task's number in the key log.
    id: u64,
    /// Times the card was found to have lost the PIN's verification during
    /// this task's key operation (see `KeySession::refused`).
    pin_losses: AtomicU32,
    purpose: KeyPurpose,
    listener: Arc<dyn StageListener<W>>,
    cancelled: AtomicBool,
    panicked: AtomicBool,
    inner: Mutex<Inner<W>>,
    changed: Condvar,
}

/// Whether `new` would show the person nothing new.
fn same<W: Workflow>(old: &Stage<W>, new: &Stage<W>) -> bool {
    use Stage as S;
    match (old, new) {
        (S::PlugIn, S::PlugIn) | (S::Busy, S::Busy) | (S::Touch, S::Touch) => true,
        (S::Working { progress: a }, S::Working { progress: b }) => a == b,
        (
            S::Pin { tries_left: a, forgot_after_background: b, last_error: c },
            S::Pin { tries_left: x, forgot_after_background: y, last_error: z },
        ) => a == x && b == y && c == z,
        _ => false,
    }
}

impl<W: Workflow> TaskCore<W> {
    fn lock(&self) -> MutexGuard<'_, Inner<W>> {
        // A listener that panicked did so outside this lock.
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Writes a stage change to the key log of the session the task is on.
    fn log_stage(&self, session: Option<Arc<KeySession>>, state: &Stage<W>) {
        if let (Some(session), Some(words)) = (session, super::keylog::stage_words(state)) {
            let log = session.log();
            log.line(format!("{} stage {words}", self.tag()));
            if matches!(state, Stage::Pin { .. }) {
                log.pin_asked();
            } else if state.is_final() {
                log.task_ended();
            }
        }
    }

    /// `again` delivers a stage even if it looks like the current one: a
    /// PIN asked again after a refusal is news, even with the same counter.
    fn apply(inner: &mut Inner<W>, new: Stage<W>, again: bool) -> bool {
        if inner.state.is_final() || (!again && same(&inner.state, &new)) {
            return false;
        }
        if !matches!(new, Stage::Pin { .. }) {
            inner.supplied = None;
        }
        inner.state = new.clone();
        inner.queue.push_back(new);
        true
    }

    /// Moves to `new` unless the task has finished. Returns whether it did.
    pub(crate) fn set(&self, new: Stage<W>) -> bool {
        self.set_again(new, false)
    }

    fn set_again(&self, new: Stage<W>, again: bool) -> bool {
        let (applied, session) = {
            let mut inner = self.lock();
            let applied = Self::apply(&mut inner, new.clone(), again);
            (applied, if applied { inner.session.clone() } else { None })
        };
        if applied {
            self.log_stage(session, &new);
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
            self.listener.changed(next);
        }
    }

    fn cancel(&self) {
        let session = {
            let mut inner = self.lock();
            if inner.committing || inner.state.is_final() {
                return;
            }
            self.cancelled.store(true, Ordering::SeqCst);
            Self::apply(&mut inner, Stage::Cancelled, false);
            inner.session.clone()
        };
        self.log_stage(session.clone(), &Stage::Cancelled);
        self.changed.notify_all();
        self.drain();
        if let Some(session) = session {
            session.wake_card_waiters();
        }
    }

    pub(crate) fn bind(self: &Arc<Self>, session: &Arc<KeySession>) {
        self.lock().session = Some(session.clone());
        let side: Arc<dyn TaskSide> = self.clone();
        session.watch(&side);
    }

    /// Waits until `done` says so, or the task is cancelled.
    pub(crate) fn wait_until(&self, mut done: impl FnMut() -> bool) -> Result<(), Halt> {
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

    /// Shows the PIN stage until a PIN of a plausible length is supplied.
    fn ask_pin(&self, session: &KeySession, mut last_error: Option<KeyError>) -> Result<Pin, Halt> {
        loop {
            let prompt = session.pin_prompt();
            if prompt.tries_left == 0 {
                return Err(Halt::Fail(KeyError::PinBlocked));
            }
            let again = last_error.is_some();
            let stage = Stage::Pin {
                tries_left: prompt.tries_left,
                forgot_after_background: prompt.forgot_after_background,
                last_error: last_error.take(),
            };
            self.set_again(stage, again);
            let mut supplied = {
                let mut inner = self.lock();
                loop {
                    if self.cancelled() {
                        return Err(Halt::Fail(KeyError::Cancelled));
                    }
                    if session.ended() {
                        return Err(Halt::Fail(KeyError::KeyRemoved));
                    }
                    if let Some(pin) = inner.supplied.take() {
                        break pin;
                    }
                    inner = self.changed.wait(inner).unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            };
            match Pin::new(std::mem::take(&mut *supplied)) {
                Ok(pin) => return Ok(pin),
                Err(e) => last_error = Some(e),
            }
        }
    }

    /// Runs `op` in a card job with the PIN verified, asking the person for
    /// it first if paw holds none, and again after a wrong one.
    pub(crate) fn unlocked<T>(
        &self,
        session: &KeySession,
        mut op: impl FnMut(&mut super::session::CardJob<'_>) -> Result<T, Halt>,
    ) -> Result<T, Halt> {
        let mut last_error = None;
        loop {
            let supplied =
                if session.holds_pin_now() { None } else { Some(self.ask_pin(session, last_error.take())?) };
            match session.run(self, |job| {
                job.unlock(supplied)?;
                op(job)
            }) {
                Err(Halt::AskPin(why)) => last_error = why,
                other => return other,
            }
        }
    }
}

impl<W: Workflow> TaskSide for TaskCore<W> {
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

    fn show_busy(&self) {
        self.set(Stage::Busy);
    }

    fn show_touch(&self) {
        self.set(Stage::Touch);
    }

    fn show_working(&self) {
        self.set(Stage::Working { progress: None });
    }

    fn report_progress(&self, done: u32, total: u32) -> bool {
        if self.cancelled() || self.lock().state.is_final() {
            return false;
        }
        self.set(Stage::Working { progress: Some(TaskProgress { done, total }) });
        !self.cancelled()
    }

    fn begin_commit(&self) -> bool {
        let mut inner = self.lock();
        if self.cancelled() || inner.state.is_final() {
            return false;
        }
        inner.committing = true;
        true
    }
}

/// One operation that needs the key, running on its own thread. See the
/// module comment.
pub struct Running<W: Workflow> {
    core: Arc<TaskCore<W>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// What a task's thread runs.
pub(crate) type Body<W> = Box<
    dyn FnOnce(&Arc<TaskCore<W>>) -> Result<<W as Workflow>::Outcome, Failure<<W as Workflow>::Failure>> + Send,
>;

impl<W: Workflow> Running<W> {
    /// Creates a task in `initial` and, unless that is final, starts its
    /// thread on `body`.
    pub(crate) fn start(
        purpose: KeyPurpose,
        listener: Arc<dyn StageListener<W>>,
        initial: Stage<W>,
        body: Body<W>,
    ) -> Self {
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
        let task = Self { core: core.clone(), thread: Mutex::new(None) };
        if finished {
            return task;
        }
        let spawned = std::thread::Builder::new().name("paw-key-task".into()).spawn(move || {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| body(&core)));
            let end = match result {
                Ok(result) => Stage::ended(result),
                Err(_) => {
                    core.panicked.store(true, Ordering::SeqCst);
                    Stage::Failed(KeyError::Internal { detail: "paw hit an internal error".into() })
                }
            };
            core.set(end);
        });
        match spawned {
            Ok(handle) => *task.thread.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle),
            Err(e) => {
                task.core.set(Stage::Failed(KeyError::Internal { detail: e.to_string() }));
            }
        }
        task
    }

    /// A task that has already failed with `failure`.
    pub(crate) fn failed(
        purpose: KeyPurpose,
        listener: Arc<dyn StageListener<W>>,
        failure: Failure<W::Failure>,
    ) -> Self {
        Self::start(purpose, listener, Stage::ended(Err(failure)), Box::new(|_| unreachable!("a failed task never runs")))
    }

    /// "#3": the task in the key log.
    pub(crate) fn tag(&self) -> String {
        self.core.tag()
    }

    pub fn purpose(&self) -> KeyPurpose {
        self.core.purpose.clone()
    }

    /// The stage to show now.
    pub fn stage(&self) -> Stage<W> {
        self.core.lock().state.clone()
    }

    /// The PIN as typed, as bytes. Ignored (and wiped) unless the task is
    /// at the PIN stage.
    pub fn supply_pin(&self, pin: Vec<u8>) {
        let pin = Zeroizing::new(pin);
        let mut inner = self.core.lock();
        if matches!(inner.state, Stage::Pin { .. }) {
            inner.supplied = Some(pin);
            self.core.changed.notify_all();
        }
    }

    /// Stops the task now, at any stage. See the module comment.
    pub fn cancel(&self) {
        self.core.cancel();
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

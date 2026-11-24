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
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use super::enrolment::KeyMismatch;
use super::session::{Halt, KeySession, Pin};
use super::signer::SessionSigner;
use super::{KeyError, KeyOutcome, KeyPurpose, KeyTaskListener, KeyTaskState, TaskProgress};

struct Inner {
    state: KeyTaskState,
    /// States not yet given to the listener.
    queue: VecDeque<KeyTaskState>,
    /// Some thread is giving `queue` to the listener.
    draining: bool,
    /// A PIN supplied at the PIN stage, not yet taken.
    supplied: Option<Zeroizing<Vec<u8>>>,
    /// Past the point where cancel can still stop it.
    committing: bool,
    /// The session the task runs on, once it has one.
    session: Option<Arc<KeySession>>,
    /// A sign-in task has the PIN verified and can sign.
    signer_ready: bool,
    /// The task refused a card that is not the enrolled key.
    mismatch: Option<KeyMismatch>,
    /// How far the clone or fetch after a sign-in has got.
    transfer: Option<crate::sync::TransferProgress>,
}

/// Numbers tasks in the key log, per process.
static NEXT_TASK: AtomicU64 = AtomicU64::new(1);

pub(crate) struct TaskCore {
    /// The task's number in the key log.
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

/// Whether `new` would show the person nothing new.
fn same(old: &KeyTaskState, new: &KeyTaskState) -> bool {
    use KeyTaskState as S;
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

impl TaskCore {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A listener that panicked did so outside this lock.
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// "#3": the task in the key log.
    pub(crate) fn tag(&self) -> String {
        format!("#{}", self.id)
    }

    /// Counts one more loss of the PIN's verification; returns how many
    /// this task has seen.
    pub(crate) fn note_pin_lost(&self) -> u32 {
        self.pin_losses.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Writes a stage change to the key log of the session the task is on.
    fn log_stage(&self, session: Option<Arc<KeySession>>, state: &KeyTaskState) {
        if let (Some(session), Some(words)) = (session, super::keylog::stage_words(state)) {
            let log = session.log();
            log.line(format!("{} stage {words}", self.tag()));
            if matches!(state, KeyTaskState::Pin { .. }) {
                log.pin_asked();
            } else if state.is_final() {
                log.task_ended();
            }
        }
    }

    pub(crate) fn wake(&self) {
        let _inner = self.lock();
        self.changed.notify_all();
    }

    /// `again` delivers a stage even if it looks like the current one: a
    /// PIN asked again after a refusal is news, even with the same counter.
    fn apply(inner: &mut Inner, new: KeyTaskState, again: bool) -> bool {
        if inner.state.is_final() || (!again && same(&inner.state, &new)) {
            return false;
        }
        if !matches!(new, KeyTaskState::Pin { .. }) {
            inner.supplied = None;
        }
        inner.state = new.clone();
        inner.queue.push_back(new);
        true
    }

    /// Moves to `new` unless the task has finished. Returns whether it did.
    pub(crate) fn set(&self, new: KeyTaskState) -> bool {
        self.set_again(new, false)
    }

    fn set_again(&self, new: KeyTaskState, again: bool) -> bool {
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
            inner.session.clone()
        };
        self.log_stage(session.clone(), &KeyTaskState::Cancelled);
        self.changed.notify_all();
        self.drain();
        if let Some(session) = session {
            session.wake_card_waiters();
        }
    }

    /// The clone or fetch after a sign-in has got this far: shown as
    /// `Working` with objects done and expected, and kept for
    /// `transfer_progress`. Nothing once the task has been cancelled or has
    /// finished.
    pub(crate) fn report_transfer(&self, update: crate::sync::TransferProgress) -> bool {
        if self.cancelled() {
            return false;
        }
        {
            let mut inner = self.lock();
            if inner.state.is_final() {
                return false;
            }
            inner.transfer = Some(update);
        }
        self.set(KeyTaskState::Working {
            progress: Some(TaskProgress { done: update.received_objects, total: update.total_objects }),
        });
        !self.cancelled()
    }

    /// From here on, cancel is too late. `false` if it already came.
    pub(crate) fn begin_commit(&self) -> bool {
        let mut inner = self.lock();
        if self.cancelled() || inner.state.is_final() {
            return false;
        }
        inner.committing = true;
        true
    }

    pub(crate) fn bind(self: &Arc<Self>, session: &Arc<KeySession>) {
        self.lock().session = Some(session.clone());
        session.watch(self);
    }

    /// Records why the card was refused, before the task says it failed.
    pub(crate) fn set_key_mismatch(&self, mismatch: KeyMismatch) {
        self.lock().mismatch = Some(mismatch);
    }

    pub(crate) fn mark_signer_ready(&self) {
        self.lock().signer_ready = true;
        self.changed.notify_all();
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
            let stage = KeyTaskState::Pin {
                tries_left: (prompt.tries_left < 3).then_some(prompt.tries_left),
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
        self: &Arc<Self>,
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

/// One operation that needs the key. See the module comment.
#[derive(uniffi::Object)]
pub struct KeyTask {
    core: Arc<TaskCore>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// What a task's thread runs. `Ok(None)` leaves the task running (a
/// sign-in, which its caller finishes).
pub(crate) type Body = Box<dyn FnOnce(&Arc<TaskCore>) -> Result<Option<KeyOutcome>, Halt> + Send>;

impl KeyTask {
    /// Creates a task in `initial` and, unless that is final, starts its
    /// thread on `body`.
    ///
    /// `mismatch` is set when the task starts out refusing a card that is
    /// not the enrolled key.
    pub(crate) fn start_with_mismatch(
        purpose: KeyPurpose,
        listener: Arc<dyn KeyTaskListener>,
        initial: KeyTaskState,
        mismatch: Option<KeyMismatch>,
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
                signer_ready: false,
                mismatch,
                transfer: None,
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
                Ok(Ok(Some(outcome))) => Some(KeyTaskState::Done { outcome }),
                Ok(Ok(None)) => None,
                Ok(Err(Halt::Fail(error))) => Some(KeyTaskState::Failed { error }),
                Ok(Err(Halt::AskPin(_))) => Some(KeyTaskState::Failed {
                    error: KeyError::Reader { detail: "the PIN was needed where it cannot be asked".into() },
                }),
                Err(_) => {
                    core.panicked.store(true, Ordering::SeqCst);
                    Some(KeyTaskState::Failed { error: KeyError::Reader { detail: "paw hit an internal error".into() } })
                }
            };
            if let Some(end) = end {
                core.set(end);
            }
        });
        match spawned {
            Ok(handle) => *task.thread.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle),
            Err(e) => {
                task.core.set(KeyTaskState::Failed { error: KeyError::Reader { detail: e.to_string() } });
            }
        }
        task
    }

    /// "#3": the task in the key log.
    pub(crate) fn tag(&self) -> String {
        self.core.tag()
    }

    /// A task that has already failed with `error`.
    pub(crate) fn failed(purpose: KeyPurpose, listener: Arc<dyn KeyTaskListener>, error: KeyError) -> Arc<Self> {
        Self::start_with_mismatch(purpose, listener, KeyTaskState::Failed { error }, None, Box::new(|_| Ok(None)))
    }

    /// Blocks until the task has finished, or `timeout` has passed. For
    /// tests and Rust callers; Swift uses the listener.
    pub fn wait_final(&self, timeout: Duration) -> Option<KeyTaskState> {
        let deadline = Instant::now() + timeout;
        let mut inner = self.core.lock();
        loop {
            if inner.state.is_final() {
                return Some(inner.state.clone());
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            inner = self.core.changed.wait_timeout(inner, left).unwrap_or_else(|p| p.into_inner()).0;
        }
    }

    /// Waits for the task's thread to end. `true` if it panicked (it then
    /// failed with a reader error rather than hang).
    pub fn join(&self) -> bool {
        let handle = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        self.core.panicked.load(Ordering::SeqCst)
    }

    /// For a sign-in task: waits until the PIN is verified, then returns
    /// the signer to open the git connection with. Call it before
    /// connecting, so that no PIN is asked while the server waits.
    pub fn login_signer(&self) -> Result<SessionSigner, KeyError> {
        let mut inner = self.core.lock();
        loop {
            match &inner.state {
                KeyTaskState::Failed { error } => return Err(error.clone()),
                KeyTaskState::Cancelled => return Err(KeyError::Cancelled),
                KeyTaskState::Done { .. } => {
                    return Err(KeyError::Reader { detail: "the sign-in has already finished".into() });
                }
                _ => {}
            }
            if let (true, Some(session)) = (inner.signer_ready, inner.session.clone()) {
                drop(inner);
                return SessionSigner::new(self.core.clone(), session);
            }
            inner = self.core.changed.wait(inner).unwrap_or_else(|p| p.into_inner());
        }
    }

    /// For a sign-in task: the work after the login has got this far.
    pub fn report_progress(&self, done: u32, total: u32) {
        self.core.set(KeyTaskState::Working { progress: Some(TaskProgress { done, total }) });
    }

    /// For a sign-in task: the caller is done with the key. How the network
    /// part went is the caller's to report; the key's part is done.
    pub fn finish_login(&self) {
        self.core.set(KeyTaskState::Done { outcome: KeyOutcome::SignedIn });
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

    /// If the task failed because the plugged-in key is not the one the
    /// store was set up with: both keys, for "This is a different key from
    /// the one this store was set up with". The task's error is then
    /// `NotForThisKey`, and no PIN was sent.
    pub fn key_mismatch(&self) -> Option<KeyMismatch> {
        self.core.lock().mismatch.clone()
    }

    /// For a download or sync: objects and bytes received so far by its
    /// clone or fetch, at most ten updates a second (each also shown as
    /// `Working` with objects done and expected). `None` before the
    /// transfer starts. Not updated after the task is cancelled.
    pub fn transfer_progress(&self) -> Option<crate::sync::TransferProgress> {
        self.core.lock().transfer
    }
}

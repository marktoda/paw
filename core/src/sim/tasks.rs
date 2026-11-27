//! Key tasks for Rust tests and the simulator's prepared stores. The login
//! and decipher tasks are the app's own (`sign_in_then`, `decrypt_then`);
//! only the work after the key differs.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use super::SimulatedKey;
use crate::key::{KeyPort, SessionSigner, SignFailure};
use crate::key::SshSigner;
use crate::ssh::SshPublicKey;
use crate::task::{
    DecryptWork, KeyOutcome, KeyPurpose, KeyTask, KeyTaskListener, KeyTaskState, LoginPurpose, TaskFailure, Work,
};

/// How long a test waits for a stage, or a task to end.
pub const WAIT: Duration = Duration::from_secs(10);

/// Plays the person at the key sheet: records every stage, types the PIN
/// at each PIN stage (or gives up once it is refused) and touches the key
/// when asked, as it is told to.
#[derive(Default)]
pub struct SimDriver {
    states: Mutex<Vec<KeyTaskState>>,
    changed: Condvar,
    task: OnceLock<Weak<KeyTask>>,
    pin: Option<String>,
    /// Cancels the task when the PIN is asked again, instead of typing it.
    gives_up: bool,
    toucher: Option<Arc<SimulatedKey>>,
}

impl SimDriver {
    /// Watches only.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Types `pin` at the PIN stage and touches `key` at the touch stage.
    pub fn helpful(pin: &str, key: &Arc<SimulatedKey>) -> Arc<Self> {
        Arc::new(Self { pin: Some(pin.into()), toucher: Some(key.clone()), ..Self::default() })
    }

    /// Types `pin` at the PIN stage, but never touches.
    pub fn typist(pin: &str) -> Arc<Self> {
        Arc::new(Self { pin: Some(pin.into()), ..Self::default() })
    }

    /// Types `pin` once: if the key refuses it, cancels the task rather
    /// than spend another try.
    pub fn typist_once(pin: &str) -> Arc<Self> {
        Arc::new(Self { pin: Some(pin.into()), gives_up: true, ..Self::default() })
    }

    /// Touches `key` when asked, but types nothing.
    pub fn toucher(key: &Arc<SimulatedKey>) -> Arc<Self> {
        Arc::new(Self { toucher: Some(key.clone()), ..Self::default() })
    }

    /// Lets the driver type the PIN, including at a PIN stage the task
    /// started in (which the listener is not told about).
    pub fn drive(&self, task: &Arc<KeyTask>) {
        let _ = self.task.set(Arc::downgrade(task));
        if let state @ KeyTaskState::Pin { .. } = task.state() {
            self.answer(task, &state);
        }
    }

    fn answer(&self, task: &KeyTask, state: &KeyTaskState) {
        let Some(pin) = &self.pin else { return };
        if self.gives_up && matches!(state, KeyTaskState::Pin { last_error: Some(_), .. }) {
            task.cancel();
        } else {
            task.supply_pin(pin.as_bytes().to_vec());
        }
    }

    pub fn states(&self) -> Vec<KeyTaskState> {
        self.states.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn saw(&self, wanted: impl Fn(&KeyTaskState) -> bool) -> bool {
        self.states().iter().any(wanted)
    }

    pub fn count(&self, wanted: impl Fn(&KeyTaskState) -> bool) -> usize {
        self.states().iter().filter(|s| wanted(s)).count()
    }

    /// Waits until a stage `wanted` has been reported. Panics after [`WAIT`].
    pub fn wait_for(&self, wanted: impl Fn(&KeyTaskState) -> bool) {
        let deadline = Instant::now() + WAIT;
        let mut states = self.states.lock().unwrap_or_else(|p| p.into_inner());
        while !states.iter().any(&wanted) {
            let left = deadline.checked_duration_since(Instant::now()).expect("the stage never came");
            states = self.changed.wait_timeout(states, left).unwrap_or_else(|p| p.into_inner()).0;
        }
    }
}

impl KeyTaskListener for SimDriver {
    fn state_changed(&self, state: KeyTaskState) {
        match &state {
            KeyTaskState::Pin { .. } => {
                if let Some(task) = self.task.get().and_then(Weak::upgrade) {
                    self.answer(&task, &state);
                }
            }
            KeyTaskState::Touch => {
                if let Some(key) = &self.toucher {
                    key.touch();
                }
            }
            _ => {}
        }
        self.states.lock().unwrap_or_else(|p| p.into_inner()).push(state);
        self.changed.notify_all();
    }
}

/// The task's final stage, once it has one; `None` if it has none within
/// `within`.
pub fn wait_final(task: &KeyTask, within: Duration) -> Option<KeyTaskState> {
    let deadline = Instant::now() + within;
    loop {
        let state = task.state();
        if state.is_final() {
            return Some(state);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// How `ended`, a final stage, ended: its outcome, or its failure.
pub fn ending(ended: KeyTaskState) -> Result<KeyOutcome, TaskFailure> {
    match ended {
        KeyTaskState::Done { outcome } => Ok(outcome),
        KeyTaskState::Failed { error } => Err(error.into()),
        KeyTaskState::SyncFailed { error } => Err(error.into()),
        KeyTaskState::SetupFailed { error } => Err(error.into()),
        KeyTaskState::Cancelled => Err(TaskFailure::Cancelled),
        KeyTaskState::PlugIn | KeyTaskState::Pin { .. } | KeyTaskState::Busy | KeyTaskState::Touch | KeyTaskState::Working { .. } => {
            panic!("the task has not ended: {ended:?}")
        }
    }
}

/// A sign-in task whose signer the test logs in with. The task stays at
/// `Working` until [`Self::finish`] (or drop), then ends `Done`; a
/// signature that fails ends it with the key's error, as a sync does.
pub struct SimLogin {
    task: Arc<KeyTask>,
    signer: Mutex<Receiver<SessionSigner>>,
    failed: Sender<SignFailure>,
    done: Mutex<Option<Sender<()>>>,
}

/// The signer a [`SimLogin`] hands over.
pub struct SimSigner {
    signer: SessionSigner,
    failed: Sender<SignFailure>,
}

impl SshSigner for SimSigner {
    fn public_key(&self) -> &SshPublicKey {
        self.signer.public_key()
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, SignFailure> {
        self.signer.sign(data).inspect_err(|error| {
            let _ = self.failed.send(error.clone());
        })
    }
}

impl SimLogin {
    /// Starts the sign-in on `port`, refusing any key but the enrolled one.
    pub fn start(port: &Arc<KeyPort>, login: LoginPurpose, listener: Arc<dyn KeyTaskListener>) -> Self {
        let (signer_tx, signer_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let (failed_tx, failed_rx) = mpsc::channel::<SignFailure>();
        let work: Work = Box::new(move |task, signer| {
            let _ = signer_tx.send(signer);
            // Like a clone or fetch, it stops when the task is cancelled.
            loop {
                if task.cancelled() {
                    return Err(TaskFailure::Cancelled);
                }
                match failed_rx.try_recv() {
                    Ok(SignFailure::Key(error)) => return Err(error.into()),
                    Ok(SignFailure::Cancelled) => return Err(TaskFailure::Cancelled),
                    Err(_) => {}
                }
                match done_rx.recv_timeout(Duration::from_millis(5)) {
                    Err(RecvTimeoutError::Timeout) => {}
                    _ => return Ok(KeyOutcome::SignedIn),
                }
            }
        });
        let task = port.sign_in_then(login, listener, port.expected(), work);
        Self { task, signer: Mutex::new(signer_rx), failed: failed_tx, done: Mutex::new(Some(done_tx)) }
    }

    pub fn task(&self) -> &Arc<KeyTask> {
        &self.task
    }

    /// Waits until the PIN is verified, then hands over the signer (once);
    /// the task's final stage if it ended first.
    pub fn signer(&self) -> Result<SimSigner, Box<KeyTaskState>> {
        let signer = self.signer.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            let state = self.task.state();
            if state.is_final() {
                return Err(Box::new(state));
            }
            match signer.recv_timeout(Duration::from_millis(5)) {
                Ok(signer) => return Ok(SimSigner { signer, failed: self.failed.clone() }),
                Err(RecvTimeoutError::Timeout) => {}
                // The task ended before its work began: its state says why.
                Err(RecvTimeoutError::Disconnected) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }

    /// The test has finished with the key: the task ends `Done`.
    pub fn finish(&self) {
        self.done.lock().unwrap_or_else(|p| p.into_inner()).take();
    }
}

impl Drop for SimLogin {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Deciphers `ciphertext` with the key on `port` in a key task, as
/// re-encrypting a held-back change does. The plaintext arrives on the
/// receiver if the task ends `Done`; otherwise its state has the error.
pub fn decipher(
    port: &Arc<KeyPort>,
    ciphertext: Vec<u8>,
    listener: Arc<dyn KeyTaskListener>,
) -> (Arc<KeyTask>, Receiver<Zeroizing<Vec<u8>>>) {
    let (text_tx, text_rx) = mpsc::channel();
    let work: DecryptWork = Box::new(move |_, open| {
        let _ = text_tx.send(open(&ciphertext)?);
        Ok(KeyOutcome::KeyChecked)
    });
    (port.decrypt_then(KeyPurpose::Reencrypt { count: 1 }, listener, work), text_rx)
}

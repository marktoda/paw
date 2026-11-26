//! Key tasks for Rust tests whose work is their own: a git login whose
//! signer the test uses, and one ciphertext deciphered. Both are the
//! app's tasks (`sign_in_then`, `decrypt_then`); only the
//! work after the key differs.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zeroize::Zeroizing;

use crate::key::{KeyError, KeyPort, SessionSigner};
use crate::task::{DecryptWork, KeyOutcome, KeyPurpose, KeyTask, KeyTaskListener, KeyTaskState, LoginPurpose, Work};
use crate::ssh::{SshPublicKey, SshSigner};

/// A sign-in task whose signer the test logs in with. The task stays at
/// `Working` until [`Self::finish`] (or drop), then ends `Done`; a
/// signature that fails ends it with the key's error, as a sync does.
pub struct SimLogin {
    task: Arc<KeyTask>,
    signer: Mutex<Receiver<SessionSigner>>,
    failed: Sender<KeyError>,
    done: Mutex<Option<Sender<()>>>,
}

/// The signer a [`SimLogin`] hands over.
pub struct SimSigner {
    signer: SessionSigner,
    failed: Sender<KeyError>,
}

impl SshSigner for SimSigner {
    fn public_key(&self) -> &SshPublicKey {
        self.signer.public_key()
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, KeyError> {
        self.signer.sign(data).inspect_err(|error| {
            let _ = self.failed.send(error.clone());
        })
    }
}

fn ended(state: &KeyTaskState) -> Option<KeyError> {
    match state {
        KeyTaskState::Failed { error } => Some(error.clone()),
        KeyTaskState::Cancelled => Some(KeyError::Cancelled),
        KeyTaskState::Done { .. } => Some(KeyError::Internal { detail: "the sign-in has already finished".into() }),
        _ => None,
    }
}

impl SimLogin {
    /// Starts the sign-in on `port`, refusing any key but the enrolled one.
    pub fn start(port: &Arc<KeyPort>, login: LoginPurpose, listener: Arc<dyn KeyTaskListener>) -> Self {
        let (signer_tx, signer_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let (failed_tx, failed_rx) = mpsc::channel::<KeyError>();
        let work: Work = Box::new(move |task, signer| {
            let _ = signer_tx.send(signer);
            // Like a clone or fetch, it stops when the task is cancelled.
            loop {
                if task.cancelled() {
                    return Err(KeyError::Cancelled.into());
                }
                if let Ok(error) = failed_rx.try_recv() {
                    return Err(error.into());
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
    /// the task's error if it ended first.
    pub fn signer(&self) -> Result<SimSigner, KeyError> {
        let signer = self.signer.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(error) = ended(&self.task.state()) {
                return Err(error);
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

//! Where each kind of key task starts, and what it asks the key to do.
//!
//! A task started with no key connected waits at `PlugIn` and goes on by
//! itself when one is plugged in. A task keeps the session it started
//! with: if that key goes away, the task fails with `KeyRemoved`.

use std::sync::Arc;
use std::time::Duration;

use super::run::{Body, Job, TaskCore, purpose_words};
use super::{KeyOutcome, KeyPurpose, KeyTask, KeyTaskListener, KeyTaskState, LoginPurpose, TaskFailure};
use crate::PawError;
use crate::copies::CopyCache;
use crate::files::content_id;
use crate::key::{FirstStage, Halt, KeyError, KeyIdentity, KeyPort, KeySession, Plan, SessionSigner, TaskSide, authenticate};
use crate::store::PassStore;

/// The bytes the touch timeout check asks the key to sign: as many as an
/// SSH login for user `git` has it sign.
const TOUCH_CHECK_LEN: usize = 146;

/// Work a task does once the key has done its part, before it ends.
pub(crate) type After = Box<dyn FnOnce(&Arc<KeySession>) -> Result<(), TaskFailure> + Send>;

/// What a sign-in task does on the network with the signer, in the task's
/// thread. It reports progress and ends the task.
pub(crate) type Work = Box<dyn FnOnce(&Job<'_>, SessionSigner) -> Result<KeyOutcome, TaskFailure> + Send>;

/// Deciphers one ciphertext with the session's key, in a decrypting
/// task's thread: the PIN is asked once, the key is touched for each.
pub(crate) type Opener<'a> = dyn FnMut(&[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, Halt> + 'a;

/// What a decrypting task does with its [`Opener`], in the task's thread.
/// It reports progress and returns how the task ends. Once the task is
/// cancelled, the opener gives nothing more.
pub(crate) type DecryptWork =
    Box<dyn FnOnce(&Job<'_>, &mut Opener<'_>) -> Result<KeyOutcome, TaskFailure> + Send>;

/// What an open does once the entry has decrypted.
enum Then {
    /// Hands the entry over, keeping a Face ID copy if given a cache.
    Keep(Option<(Arc<CopyCache>, Duration)>),
    /// Wipes it at once (setup's key check), then runs `after`.
    Check(After),
}

fn entry_unreadable(error: PawError) -> KeyError {
    match error {
        PawError::StoreBusy => KeyError::StoreBusy,
        PawError::NotText => KeyError::EntryUnreadable { detail: "it is not text".into() },
        other => KeyError::EntryUnreadable { detail: other.to_string() },
    }
}

/// The stage a task shows first on its key.
fn first_state(first: FirstStage) -> KeyTaskState {
    match first {
        FirstStage::Pin(prompt) => KeyTaskState::Pin {
            tries_left: prompt.tries_left,
            forgot_after_background: prompt.forgot_after_background,
            last_error: None,
        },
        FirstStage::Busy => KeyTaskState::Busy,
        FirstStage::Working => KeyTaskState::Working { progress: None },
    }
}

/// Puts `task` on `session`: writes its start to the key log and shows its
/// first stage, or says why it is refused before any PIN or touch
/// ([`KeySession::preflight`]). Gives what `needs` found.
fn enter<T>(
    task: &Arc<TaskCore>,
    what: &str,
    session: &Arc<KeySession>,
    plugged_later: bool,
    expected: Option<&KeyIdentity>,
    needs: impl FnOnce(&KeySession) -> Result<T, KeyError>,
) -> Result<T, KeyError> {
    let info = session.card_info();
    session.log().line(format!(
        "{} task {what}: PIN held {}, touch decryption {}, authentication {}{}",
        task.tag(),
        if session.holds_pin_now() { "yes" } else { "no" },
        info.decryption_touch.words(),
        info.authentication_touch.words(),
        if plugged_later { ", key plugged in after it started" } else { "" },
    ));
    task.bind(session);
    let (first, found) = session.preflight(expected, needs)?;
    task.begin(first_state(first));
    Ok(found)
}

/// Starts a task on `port`'s connected key's session, or at `PlugIn` if
/// there is none. `needs` is what the task checks before any PIN or touch;
/// what it finds goes to `body`, which runs on the task's own thread.
///
/// A card that is not `expected` (the enrolled key, unless a caller such
/// as setup knows better) is refused before anything else, so no PIN
/// reaches it.
fn start<T: Send + 'static>(
    port: &Arc<KeyPort>,
    purpose: KeyPurpose,
    listener: Arc<dyn KeyTaskListener>,
    expected: Option<KeyIdentity>,
    needs: impl FnOnce(&KeySession) -> Result<T, KeyError> + Send + 'static,
    body: impl FnOnce(&Arc<TaskCore>, &Arc<KeySession>, T) -> Result<KeyOutcome, TaskFailure> + Send + 'static,
) -> Arc<KeyTask> {
    let what = purpose_words(&purpose);
    let task = KeyTask::new(purpose, listener);
    let core = task.core();
    let run: Body = match port.session() {
        Some(session) => match enter(core, what, &session, false, expected.as_ref(), needs) {
            Ok(found) => Box::new(move |task| body(task, &session, found)),
            Err(error) => {
                core.begin(KeyTaskState::Failed { error });
                return task;
            }
        },
        None => {
            core.begin(KeyTaskState::PlugIn);
            let port = port.clone();
            Box::new(move |task| {
                let side: Arc<dyn TaskSide> = task.clone();
                let session = port.wait_for_session(&side)?;
                let found = enter(task, what, &session, true, expected.as_ref(), needs)?;
                body(task, &session, found)
            })
        }
    };
    task.run(run);
    task
}

impl KeyPort {
    /// A sign-in task that, once the PIN is verified, runs `work` with a
    /// signer in the task's own thread: setup's download and sync's send
    /// and refresh. `work` reports progress, stops between its network
    /// steps once cancelled, and returns how the task ends.
    pub(crate) fn sign_in_then(
        self: &Arc<Self>,
        login: LoginPurpose,
        listener: Arc<dyn KeyTaskListener>,
        expected: Option<KeyIdentity>,
        work: Work,
    ) -> Arc<KeyTask> {
        start(
            self,
            KeyPurpose::SignIn { login },
            listener,
            expected,
            |session| session.ssh_key().cloned(),
            move |task, session, public_key| {
                session.unlocked(task.as_ref(), |_| Ok(()))?;
                if task.cancelled() {
                    return Err(TaskFailure::Cancelled);
                }
                task.show_working();
                let signer = SessionSigner::new(task.clone(), session.clone(), public_key);
                work(&Job::of(task), signer)
            },
        )
    }

    /// A task that, once the PIN is verified, deciphers several ciphertexts
    /// with the key and hands each one's text to `work` (re-encrypting
    /// held-back changes). Refused before the PIN on a key with no
    /// decryption key.
    pub(crate) fn decrypt_then(self: &Arc<Self>, listener: Arc<dyn KeyTaskListener>, work: DecryptWork) -> Arc<KeyTask> {
        let expected = self.expected();
        start(
            self,
            KeyPurpose::Reencrypt,
            listener,
            expected,
            |session| session.decryption_key().cloned(),
            move |task, session, key| {
                session.unlocked(task.as_ref(), |_| Ok(()))?;
                if task.cancelled() {
                    return Err(TaskFailure::Cancelled);
                }
                let side: Arc<dyn TaskSide> = task.clone();
                let mut open = |ciphertext: &[u8]| -> Result<zeroize::Zeroizing<Vec<u8>>, Halt> {
                    let plan = Plan::new(ciphertext, &key)?;
                    let session_key = session.unlocked(task.as_ref(), |job| plan.decipher(job, &side, &key))?;
                    // A late answer is not written anywhere once cancelled.
                    if task.cancelled() {
                        return Err(Halt::Cancelled);
                    }
                    Ok(plan.finish(session_key)?)
                };
                work(&Job::of(task), &mut open)
            },
        )
    }

    /// Setup's key check, which then runs `after` (still in the task, past
    /// the point where cancel can stop it) before the task is done.
    pub(crate) fn check_key_then(
        self: &Arc<Self>,
        store: Arc<PassStore>,
        entry: String,
        listener: Arc<dyn KeyTaskListener>,
        expected: Option<KeyIdentity>,
        after: After,
    ) -> Arc<KeyTask> {
        let purpose = KeyPurpose::CheckKey { entry: Some(entry.clone()) };
        self.open_or_check(purpose, store, entry, Then::Check(after), listener, expected)
    }

    fn open_or_check(
        self: &Arc<Self>,
        purpose: KeyPurpose,
        store: Arc<PassStore>,
        entry: String,
        then: Then,
        listener: Arc<dyn KeyTaskListener>,
        expected: Option<KeyIdentity>,
    ) -> Arc<KeyTask> {
        let read = entry.clone();
        start(
            self,
            purpose,
            listener,
            expected,
            // The file can be read, the key has a decryption key, and the
            // entry is for it.
            move |session| {
                let ciphertext = store.read_entry(&read).map_err(entry_unreadable)?;
                let key = session.decryption_key()?;
                Plan::new(&ciphertext, key)?;
                Ok((ciphertext, key.clone()))
            },
            move |task, session, (ciphertext, key)| {
                let plan = Plan::new(&ciphertext, &key)?;
                let side: Arc<dyn TaskSide> = task.clone();
                let session_key = session.unlocked(task.as_ref(), |job| plan.decipher(job, &side, &key))?;
                // The late answer of a cancelled task is wiped on drop.
                Job::of(task).commit()?;
                let plaintext = plan.finish(session_key)?;
                let opened = crate::entry::opened::OpenedEntry::decrypted(&entry, plaintext, content_id(&ciphertext)).map_err(entry_unreadable)?;
                let copies = match then {
                    Then::Keep(copies) => copies,
                    Then::Check(after) => {
                        opened.close();
                        after(session)?;
                        return Ok(KeyOutcome::KeyChecked);
                    }
                };
                // The entry opens even if keeping the copy fails: the
                // person has already touched the key.
                let (face_id_until_ms, copy_failed) =
                    match copies.map(|(copies, ttl)| crate::open::keep_copy(&copies, &opened, ttl)) {
                        None => (None, None),
                        Some(Ok(until)) => (until, None),
                        Some(Err(error)) => (None, Some(error)),
                    };
                Ok(KeyOutcome::Opened { entry: opened, face_id_until_ms, copy_failed })
            },
        )
    }
}

#[uniffi::export]
impl KeyPort {
    /// Opens `entry` with the key and keeps a Face ID copy of it for
    /// `ttl_secs` in `copies` (none if `copies` is absent or `ttl_secs` is
    /// zero; at most [`crate::copies::MAX_TTL`]).
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
        let purpose = KeyPurpose::OpenEntry { entry: entry.clone() };
        let expected = self.expected();
        self.open_or_check(purpose, store, entry, Then::Keep(copies), listener, expected)
    }

    /// Key diagnostics' touch timeout check: with the PIN (asked if the
    /// session holds none), the key is asked for one signature with its
    /// authentication key, of fixed bytes the size of a git login, and
    /// nobody is meant to touch it. Shows `Touch` as a sign-in does.
    ///
    /// A key that waits for a touch fails the task with `TouchTimeout` once
    /// it gives up (about 15 seconds on a YubiKey); the key log then has the
    /// status word, how long the key waited, and what the PIN check after
    /// it answered. `Done` with `SignedIn` means the key signed: it was
    /// touched, or the slot asks for no touch. Nothing else is sent, and
    /// the signature is thrown away.
    ///
    /// Refused before the PIN, as every task is, on a card that is not the
    /// enrolled key, and on a key with no authentication key.
    pub fn check_touch_timeout(self: Arc<Self>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        let expected = self.expected();
        start(
            &self,
            KeyPurpose::CheckTouchTimeout,
            listener,
            expected,
            |session| session.ssh_key().map(drop),
            |task, session, ()| {
                let unsigned: Vec<u8> = (0..TOUCH_CHECK_LEN).map(|i| i as u8).collect();
                let side: Arc<dyn TaskSide> = task.clone();
                session.unlocked(task.as_ref(), |job| authenticate(job, &side, &unsigned))?;
                Ok(KeyOutcome::SignedIn)
            },
        )
    }
}

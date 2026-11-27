//! Where each kind of key task starts, and what it asks the key to do.
//!
//! A task started with no key connected waits at `PlugIn` and goes on by
//! itself when one is plugged in. A task keeps the session it started
//! with: if that key goes away, the task fails with `KeyRemoved`.

use std::sync::Arc;
use std::time::Duration;

use super::run::{Body, Job, TaskCore, purpose_words, stage_words};
use super::{KeyOutcome, KeyPurpose, KeyTask, KeyTaskListener, KeyTaskState, LoginPurpose, TaskFailure};
use crate::PawError;
use crate::copies::CopyCache;
use crate::files::content_id;
use crate::key::keylog::touch_words;
use crate::key::{Halt, KeyError, KeyIdentity, KeyPort, KeySession, Plan, SessionSigner, TaskSide, authenticate};
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

/// What opening an entry needs, checked before any PIN or touch: the file
/// can be read, the key has a decryption key, and the entry is for it.
fn read_and_check(store: &PassStore, entry: &str, session: &KeySession) -> Result<Vec<u8>, KeyError> {
    let ciphertext = store.read_entry(entry).map_err(entry_unreadable)?;
    Plan::new(&ciphertext, session.decryption_key()?)?;
    Ok(ciphertext)
}

/// Where a new task starts: the first stage that still needs doing.
/// `check` runs what can be refused before any PIN or touch.
fn initial(
    session: Option<&KeySession>,
    expected: Option<&KeyIdentity>,
    check: impl FnOnce(&KeySession) -> Result<(), KeyError>,
) -> KeyTaskState {
    let Some(session) = session else { return KeyTaskState::PlugIn };
    if let Some(error) = KeyPort::refusal(expected, session) {
        return KeyTaskState::Failed { error };
    }
    if let Err(error) = check(session) {
        return KeyTaskState::Failed { error };
    }
    if session.holds_pin_now() {
        return if session.card_busy() { KeyTaskState::Busy } else { KeyTaskState::Working { progress: None } };
    }
    let prompt = session.pin_prompt();
    if prompt.tries_left == 0 {
        return KeyTaskState::Failed { error: KeyError::PinBlocked };
    }
    KeyTaskState::Pin {
        tries_left: prompt.tries_left,
        forgot_after_background: prompt.forgot_after_background,
        last_error: None,
    }
}

/// Starts a task on `port`'s connected key's session, or at `PlugIn` if
/// there is none.
///
/// A card that is not `expected` (the enrolled key, unless a caller such
/// as setup knows better) is refused before anything else, so no PIN
/// reaches it.
fn start(
    port: &Arc<KeyPort>,
    purpose: KeyPurpose,
    listener: Arc<dyn KeyTaskListener>,
    expected: Option<KeyIdentity>,
    check: impl FnOnce(&KeySession) -> Result<(), KeyError>,
    body: impl FnOnce(&Arc<TaskCore>, &Arc<KeySession>) -> Result<KeyOutcome, TaskFailure> + Send + 'static,
) -> Arc<KeyTask> {
    let session = port.session();
    let held_at_start = session.as_deref().is_some_and(KeySession::holds_pin_now);
    let initial = initial(session.as_deref(), expected.as_ref(), check);
    let what = purpose_words(&purpose);
    let starts_at = stage_words(&initial).unwrap_or_default();
    let finished = initial.is_final();
    let starts_at_pin = matches!(initial, KeyTaskState::Pin { .. });
    let start_line = move |tag: String, session: &KeySession, plugged_later: bool| {
        let info = session.card_info();
        format!(
            "{tag} task {what}: PIN held {}, touch decryption {}, authentication {}, {}",
            if held_at_start { "yes" } else { "no" },
            touch_words(info.decryption_touch),
            touch_words(info.authentication_touch),
            if plugged_later { "key plugged in after it started".to_string() } else { format!("starts at {starts_at}") },
        )
    };
    let logged_session = session.clone();
    let port = port.clone();
    let line = start_line.clone();
    let body: Body = Box::new(move |task| {
        let session = match session {
            Some(session) => {
                session.log().line(line(task.tag(), &session, false));
                if starts_at_pin {
                    session.log().pin_asked();
                }
                session
            }
            None => {
                let side: Arc<dyn TaskSide> = task.clone();
                let session = port.wait_for_session(&side)?;
                session.log().line(line(task.tag(), &session, true));
                session
            }
        };
        task.bind(&session);
        if let Some(error) = KeyPort::refusal(expected.as_ref(), &session) {
            return Err(error.into());
        }
        body(task, &session)
    });
    let task = KeyTask::start(purpose, listener, initial, body);
    // A task refused before it began never runs its body.
    if let (true, Some(session)) = (finished, logged_session) {
        session.log().line(start_line(task.tag(), &session, false));
    }
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
            |session| session.ssh_key().map(drop),
            move |task, session| {
                session.ssh_key()?;
                session.unlocked(task.as_ref(), |_| Ok(()))?;
                if task.cancelled() {
                    return Err(TaskFailure::Cancelled);
                }
                task.show_working();
                let signer = SessionSigner::new(task.clone(), session.clone())?;
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
            |session| session.decryption_key().map(drop),
            move |task, session| {
                let key = session.decryption_key()?.clone();
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
        let check_store = store.clone();
        let check_entry = entry.clone();
        start(
            self,
            purpose,
            listener,
            expected,
            move |session| read_and_check(&check_store, &check_entry, session).map(drop),
            move |task, session| {
                let ciphertext = read_and_check(&store, &entry, session)?;
                let key = session.decryption_key()?;
                let plan = Plan::new(&ciphertext, key)?;
                let side: Arc<dyn TaskSide> = task.clone();
                let session_key = session.unlocked(task.as_ref(), |job| plan.decipher(job, &side, key))?;
                // The late answer of a cancelled task is wiped on drop.
                Job::of(task).commit()?;
                let plaintext = plan.finish(session_key)?;
                let opened = crate::open::opened(plaintext, content_id(&ciphertext)).map_err(entry_unreadable)?;
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
                    match copies.map(|(copies, ttl)| crate::open::keep_copy(&copies, &entry, &opened, ttl)) {
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
            |task, session| {
                session.ssh_key()?;
                let unsigned: Vec<u8> = (0..TOUCH_CHECK_LEN).map(|i| i as u8).collect();
                let side: Arc<dyn TaskSide> = task.clone();
                session.unlocked(task.as_ref(), |job| authenticate(job, &side, &unsigned))?;
                Ok(KeyOutcome::SignedIn)
            },
        )
    }
}

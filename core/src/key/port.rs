//! Where the key is plugged in, and where key tasks start.
//!
//! The app keeps one [`KeyPort`] for its lifetime. Plugging a key in makes a
//! [`KeySession`]; unplugging ends it and drops the PIN. A task started
//! with no key connected waits at `PlugIn` and goes on by itself when one
//! is plugged in.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use super::decrypt::Plan;
use super::enrolment::{self, KeyIdentity, KeyMismatch};
use super::keylog::{self, Sink};
use super::session::{Halt, KeySession};
use super::signer::{SessionSigner, authenticate};
use super::task::{Body, Failure, KeyDone, Running, Stage, StageListener, TaskCore, TaskSide, Workflow};
use super::{KeyError, KeyPurpose, LoginPurpose};
use super::CardConnection;
use super::Uptime;
use crate::PawError;
use crate::copies::CopyCache;
use crate::files::content_id;
use crate::entry::opened::OpenedEntry;
use crate::store::PassStore;

struct State {
    session: Option<Arc<KeySession>>,
    /// Tasks at `PlugIn`.
    waiting: Vec<Weak<dyn TaskSide>>,
    /// The key the store was set up with. While set, no task sends the PIN
    /// to another card.
    enrolled: Option<KeyIdentity>,
    /// Where this process writes its key log, once set.
    log: Option<Arc<Sink>>,
}

/// The bytes the touch timeout check asks the key to sign: as many as an
/// SSH login for user `git` has it sign.
const TOUCH_CHECK_LEN: usize = 146;

/// Work a task does once the key has done its part, before it ends.
pub(crate) type After<W> =
    Box<dyn FnOnce(&Arc<KeySession>) -> Result<(), Failure<<W as Workflow>::Failure>> + Send>;

/// How a task's work ends.
pub(crate) type Ends<W> = Result<<W as Workflow>::Outcome, Failure<<W as Workflow>::Failure>>;

/// What a sign-in task does on the network with the signer, in the task's
/// thread. It reports progress and ends the task.
pub(crate) type Work<W> = Box<dyn FnOnce(&dyn TaskSide, SessionSigner) -> Ends<W> + Send>;

/// Deciphers one ciphertext with the session's key, in a decrypting
/// task's thread: the PIN is asked once, the key is touched for each.
pub(crate) type Opener<'a> = dyn FnMut(&[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, Halt> + 'a;

/// What a decrypting task does with its [`Opener`], in the task's thread.
/// It reports progress, checks for cancel between ciphertexts, and returns
/// how the task ends.
pub(crate) type DecryptWork<W> = Box<dyn FnOnce(&dyn TaskSide, &mut Opener<'_>) -> Ends<W> + Send>;

/// What an open does once the entry has decrypted.
enum Then<W: Workflow> {
    /// Hands the entry over, keeping a Face ID copy if given a cache.
    Keep(Option<(Arc<CopyCache>, Duration)>),
    /// Wipes it at once (setup's key check), then runs `after`.
    Check(After<W>),
}

/// The refusal for `session` if it is not `expected`.
fn enrolment_check(expected: Option<&KeyIdentity>, session: &KeySession) -> Option<KeyMismatch> {
    expected.and_then(|expected| expected.check(&KeyIdentity::of(session.card_info())))
}

#[derive(uniffi::Object)]
pub struct KeyPort {
    uptime: Arc<dyn Uptime>,
    /// Counts plug-ins, so a task at `PlugIn` cannot miss one.
    plugged: AtomicU64,
    state: Mutex<State>,
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

impl KeyPort {
    /// `uptime` measures time in the background for the PIN limit.
    pub(crate) fn with_uptime(uptime: Arc<dyn Uptime>) -> Arc<Self> {
        Arc::new(Self {
            uptime,
            plugged: AtomicU64::new(0),
            state: Mutex::new(State { session: None, waiting: Vec::new(), enrolled: None, log: None }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Plain values only.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Waits at `PlugIn` until a key is connected.
    fn wait_for_session<W: Workflow>(&self, task: &Arc<TaskCore<W>>) -> Result<Arc<KeySession>, Halt> {
        loop {
            let seen = self.plugged.load(Ordering::SeqCst);
            if let Some(session) = self.session() {
                task.bind(&session);
                return Ok(session);
            }
            task.set(Stage::PlugIn);
            let side: Arc<dyn TaskSide> = task.clone();
            self.lock().waiting.push(Arc::downgrade(&side));
            task.wait_until(|| self.plugged.load(Ordering::SeqCst) != seen)?;
        }
    }

    /// Where a new task starts: the first stage that still needs doing.
    /// `check` runs what can be refused before any PIN or touch.
    fn initial<W: Workflow>(
        session: Option<&KeySession>,
        expected: Option<&KeyIdentity>,
        check: impl FnOnce(&KeySession) -> Result<(), KeyError>,
    ) -> Stage<W> {
        let Some(session) = session else { return Stage::PlugIn };
        if let Some(mismatch) = enrolment_check(expected, session) {
            return Stage::Failed(KeyError::DifferentKey { mismatch: Box::new(mismatch) });
        }
        if let Err(error) = check(session) {
            return Stage::Failed(error);
        }
        if session.holds_pin_now() {
            return if session.card_busy() { Stage::Busy } else { Stage::Working { progress: None } };
        }
        let prompt = session.pin_prompt();
        if prompt.tries_left == 0 {
            return Stage::Failed(KeyError::PinBlocked);
        }
        Stage::Pin {
            tries_left: prompt.tries_left,
            forgot_after_background: prompt.forgot_after_background,
            last_error: None,
        }
    }

    /// Starts a task on the connected key's session, or at `PlugIn` if
    /// there is none. A task keeps the session it started with: if that
    /// key goes away, the task fails with `KeyRemoved`.
    ///
    /// A card that is not `expected` (the enrolled key, unless a caller
    /// such as setup knows better) is refused before anything else, so no
    /// PIN reaches it.
    fn start<W: Workflow>(
        self: &Arc<Self>,
        purpose: KeyPurpose,
        listener: Arc<dyn StageListener<W>>,
        expected: Option<KeyIdentity>,
        check: impl FnOnce(&KeySession) -> Result<(), KeyError>,
        body: impl FnOnce(&Arc<TaskCore<W>>, &Arc<KeySession>) -> Ends<W> + Send + 'static,
    ) -> Running<W> {
        let session = self.session();
        let held_at_start = session.as_deref().is_some_and(KeySession::holds_pin_now);
        let initial = Self::initial(session.as_deref(), expected.as_ref(), check);
        let what = keylog::purpose_words(&purpose);
        let starts_at = keylog::stage_words(&initial).unwrap_or_default();
        let finished = initial.is_final();
        let starts_at_pin = matches!(initial, Stage::Pin { .. });
        let start_line = move |tag: String, session: &KeySession, plugged_later: bool| {
            let info = session.card_info();
            format!(
                "{tag} task {what}: PIN held {}, touch decryption {}, authentication {}, {}",
                if held_at_start { "yes" } else { "no" },
                keylog::touch_words(info.decryption_touch),
                keylog::touch_words(info.authentication_touch),
                if plugged_later { "key plugged in after it started".to_string() } else { format!("starts at {starts_at}") },
            )
        };
        let logged_session = session.clone();
        let port = self.clone();
        let line = start_line.clone();
        let body: Body<W> = Box::new(move |task| {
            let session = match session {
                Some(session) => {
                    session.log().line(line(task.tag(), &session, false));
                    if starts_at_pin {
                        session.log().pin_asked();
                    }
                    task.bind(&session);
                    session
                }
                None => {
                    let session = port.wait_for_session(task)?;
                    session.log().line(line(task.tag(), &session, true));
                    session
                }
            };
            if let Some(mismatch) = enrolment_check(expected.as_ref(), &session) {
                return Err(KeyError::DifferentKey { mismatch: Box::new(mismatch) }.into());
            }
            body(task, &session)
        });
        let task = Running::start(purpose, listener, initial, body);
        // A task refused before it began never runs its body.
        if let (true, Some(session)) = (finished, logged_session) {
            session.log().line(start_line(task.tag(), &session, false));
        }
        task
    }

    /// Opens `entry` with the key and keeps a Face ID copy of it for `ttl`
    /// in `copies` (none if `copies` is absent or `ttl` is zero; at most 12
    /// hours).
    ///
    /// An entry the key cannot open is refused before the PIN or a touch.
    pub(crate) fn open_entry_task<W: Workflow>(
        self: &Arc<Self>,
        store: Arc<PassStore>,
        entry: String,
        copies: Option<(Arc<CopyCache>, Duration)>,
        listener: Arc<dyn StageListener<W>>,
    ) -> Running<W> {
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
    pub(crate) fn touch_timeout_task<W: Workflow>(self: &Arc<Self>, listener: Arc<dyn StageListener<W>>) -> Running<W> {
        let expected = self.expected();
        self.start(
            KeyPurpose::CheckTouchTimeout,
            listener,
            expected,
            |session| session.ssh_key().map(drop),
            |task, session| {
                session.ssh_key()?;
                let unsigned: Vec<u8> = (0..TOUCH_CHECK_LEN).map(|i| i as u8).collect();
                let side: Arc<dyn TaskSide> = task.clone();
                task.unlocked(session, |job| authenticate(job, &side, &unsigned))?;
                if task.cancelled() {
                    return Err(KeyError::Cancelled.into());
                }
                Ok(KeyDone::SignedIn.into())
            },
        )
    }

    /// The key tasks must be on: the enrolled one, if any.
    pub(crate) fn expected(&self) -> Option<KeyIdentity> {
        self.lock().enrolled.clone()
    }

    /// A sign-in task that, once the PIN is verified, runs `work` with a
    /// signer in the task's own thread: setup's download and sync's send
    /// and refresh. `work` reports progress, checks for cancel between its
    /// network steps, and returns how the task ends.
    pub(crate) fn sign_in_then<W: Workflow>(
        self: &Arc<Self>,
        login: LoginPurpose,
        listener: Arc<dyn StageListener<W>>,
        expected: Option<KeyIdentity>,
        work: Work<W>,
    ) -> Running<W> {
        self.start(
            KeyPurpose::SignIn { login },
            listener,
            expected,
            |session| session.ssh_key().map(drop),
            move |task, session| {
                session.ssh_key()?;
                task.unlocked(session, |_| Ok(()))?;
                if task.cancelled() {
                    return Err(KeyError::Cancelled.into());
                }
                task.show_working();
                let signer = SessionSigner::new(task.clone(), session.clone())?;
                work(task.as_ref(), signer)
            },
        )
    }

    /// A task that deciphers several ciphertexts with the key and hands
    /// each one's text to `work` (re-encrypting held-back changes). Refused
    /// before the PIN on a key with no decryption key.
    pub(crate) fn decrypt_then<W: Workflow>(
        self: &Arc<Self>,
        purpose: KeyPurpose,
        listener: Arc<dyn StageListener<W>>,
        work: DecryptWork<W>,
    ) -> Running<W> {
        let expected = self.expected();
        self.start(
            purpose,
            listener,
            expected,
            |session| session.decryption_key().map(drop),
            move |task, session| {
                let key = session.decryption_key()?.clone();
                let side: Arc<dyn TaskSide> = task.clone();
                let mut open = |ciphertext: &[u8]| -> Result<zeroize::Zeroizing<Vec<u8>>, Halt> {
                    let plan = Plan::new(ciphertext, &key)?;
                    let session_key = task.unlocked(session, |job| plan.decipher(job, &side, &key))?;
                    if task.cancelled() {
                        return Err(Halt::Fail(KeyError::Cancelled));
                    }
                    Ok(plan.finish(session_key)?)
                };
                work(task.as_ref(), &mut open)
            },
        )
    }

    /// Setup's key check, which then runs `after` (still in the task, past
    /// the point where cancel can stop it) before the task is done.
    pub(crate) fn check_key_then<W: Workflow>(
        self: &Arc<Self>,
        store: Arc<PassStore>,
        entry: String,
        listener: Arc<dyn StageListener<W>>,
        expected: Option<KeyIdentity>,
        after: After<W>,
    ) -> Running<W> {
        let purpose = KeyPurpose::CheckKey { entry: Some(entry.clone()) };
        self.open_or_check(purpose, store, entry, Then::Check(after), listener, expected)
    }

    fn open_or_check<W: Workflow>(
        self: &Arc<Self>,
        purpose: KeyPurpose,
        store: Arc<PassStore>,
        entry: String,
        then: Then<W>,
        listener: Arc<dyn StageListener<W>>,
        expected: Option<KeyIdentity>,
    ) -> Running<W> {
        let check_store = store.clone();
        let check_entry = entry.clone();
        self.start(
            purpose,
            listener,
            expected,
            move |session| read_and_check(&check_store, &check_entry, session).map(drop),
            move |task, session| {
                let ciphertext = read_and_check(&store, &entry, session)?;
                let key = session.decryption_key()?;
                let plan = Plan::new(&ciphertext, key)?;
                let side: Arc<dyn TaskSide> = task.clone();
                let session_key = task.unlocked(session, |job| plan.decipher(job, &side, key))?;
                if task.cancelled() {
                    // The late answer of a cancelled task: wiped on drop.
                    return Err(KeyError::Cancelled.into());
                }
                let plaintext = plan.finish(session_key)?;
                if std::str::from_utf8(&plaintext).is_err() {
                    return Err(entry_unreadable(PawError::NotText).into());
                }
                let copies = match then {
                    Then::Keep(copies) => copies,
                    Then::Check(after) => {
                        drop(plaintext);
                        if !task.begin_commit() {
                            return Err(KeyError::Cancelled.into());
                        }
                        after(session)?;
                        return Ok(KeyDone::KeyChecked.into());
                    }
                };
                if !task.begin_commit() {
                    return Err(KeyError::Cancelled.into());
                }
                // The entry opens even if keeping the copy fails: the
                // person has already touched the key.
                let (face_id_until_ms, copy_failed) = match copies
                    .map(|(copies, ttl)| copies.put(&entry, &plaintext, &content_id(&ciphertext), ttl))
                {
                    None => (None, None),
                    Some(Ok(until)) => (until, None),
                    Some(Err(error)) => (None, Some(error)),
                };
                let mut plaintext = plaintext;
                let entry = OpenedEntry::decrypted(std::mem::take(&mut *plaintext), content_id(&ciphertext))
                    .map_err(entry_unreadable)?;
                Ok(KeyDone::Opened { entry, face_id_until_ms, copy_failed }.into())
            },
        )
    }
}

#[uniffi::export]
impl KeyPort {
    /// A key was plugged in: reads it (no PIN, no touch) and makes its
    /// session, ending any earlier one; a task waiting at `PlugIn` goes on.
    pub fn plugged_in(&self, connection: Arc<dyn CardConnection>) -> Result<Arc<KeySession>, KeyError> {
        self.unplugged();
        let sink = self.lock().log.clone();
        let again = self.plugged.load(Ordering::SeqCst) > 0;
        let session = KeySession::open(connection, self.uptime.clone(), sink, again)?;
        let waiting = {
            let mut state = self.lock();
            if let Some(old) = state.session.replace(session.clone()) {
                old.end();
            }
            self.plugged.fetch_add(1, Ordering::SeqCst);
            std::mem::take(&mut state.waiting)
        };
        for task in waiting.iter().filter_map(Weak::upgrade) {
            task.wake();
        }
        Ok(session)
    }

    /// The key was unplugged (the reader went away). Ends its session and
    /// drops the PIN; a task using it fails with `KeyRemoved`.
    pub fn unplugged(&self) {
        let session = self.lock().session.take();
        if let Some(session) = session {
            session.end();
        }
    }

    /// The connected key's session, if a key is connected and answering.
    pub fn session(&self) -> Option<Arc<KeySession>> {
        self.lock().session.clone().filter(|session| !session.ended())
    }

    /// Sets the key the store was set up with (from `load_enrolled_key`), or
    /// none. While set, every task on another card fails with
    /// `DifferentKey` before any PIN is sent.
    pub fn set_enrolled_key(&self, key: Option<KeyIdentity>) {
        self.lock().enrolled = key;
    }

    /// Reads the enrolled key of the store in `state_dir` and sets it.
    /// Returns it, or `None` if setup has not recorded one (then every card
    /// is accepted, as during setup).
    pub fn load_enrolled_key(&self, state_dir: String) -> Result<Option<KeyIdentity>, PawError> {
        let key = enrolment::load_enrolment(std::path::Path::new(&state_dir))?;
        self.set_enrolled_key(key.clone());
        Ok(key)
    }

    /// Writes the key log of the sessions read from now on to
    /// `key-log.<process>` in `state_dir` (only while that folder exists).
    /// `process` is "app" or "fill"; anything but letters and digits is
    /// dropped from it.
    pub fn set_key_log(&self, state_dir: String, process: String) {
        self.lock().log = Some(Sink::new(std::path::Path::new(&state_dir), &process));
    }

    /// The state directory this process's key log is written to, if set.
    pub fn key_log_dir(&self) -> Option<String> {
        self.lock().log.as_ref().map(|sink| sink.dir().to_string_lossy().into_owned())
    }

    /// How the plugged-in key differs from the enrolled one, or `None` if it
    /// is the enrolled key, no key is enrolled, or no key is plugged in.
    pub fn key_mismatch(&self) -> Option<KeyMismatch> {
        let session = self.session()?;
        enrolment_check(self.expected().as_ref(), &session)
    }

    /// Whether a key is plugged in and tasks may send it the PIN: it is the
    /// enrolled key, or none is enrolled.
    pub fn key_matches_enrolment(&self) -> bool {
        self.session().is_some() && self.key_mismatch().is_none()
    }
}

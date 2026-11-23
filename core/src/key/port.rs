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
use super::session::{Halt, KeySession};
use super::task::{Body, KeyTask, TaskCore};
use super::{KeyError, KeyOutcome, KeyPurpose, KeyTaskListener, KeyTaskState, LoginPurpose};
use crate::ApduTransport;
use crate::copies::{Clock, CopyCache, content_id};
use crate::ffi::OpenedEntry;
use crate::store::PassStore;

struct State {
    session: Option<Arc<KeySession>>,
    /// Tasks at `PlugIn`.
    waiting: Vec<Weak<TaskCore>>,
}

#[derive(uniffi::Object)]
pub struct KeyPort {
    clock: Arc<dyn Clock>,
    /// Counts plug-ins, so a task at `PlugIn` cannot miss one.
    plugged: AtomicU64,
    state: Mutex<State>,
}

fn entry_unreadable(error: crate::PawError) -> KeyError {
    KeyError::EntryUnreadable { detail: error.to_string() }
}

/// What opening an entry needs, checked before any PIN or touch: the file
/// can be read, the key has a decryption key, and the entry is for it.
fn read_and_check(store: &PassStore, entry: &str, session: &KeySession) -> Result<Vec<u8>, KeyError> {
    let ciphertext = store.read_entry(entry).map_err(entry_unreadable)?;
    Plan::new(&ciphertext, session.decryption_key()?)?;
    Ok(ciphertext)
}

impl KeyPort {
    fn lock(&self) -> MutexGuard<'_, State> {
        // Plain values only.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A key was plugged in: reads it (no PIN, no touch) and makes its
    /// session, ending any earlier one.
    pub fn attach(&self, transport: Arc<dyn ApduTransport>) -> Result<Arc<KeySession>, KeyError> {
        self.unplugged();
        let session = KeySession::open(transport, self.clock.clone())?;
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

    /// Waits at `PlugIn` until a key is connected.
    fn wait_for_session(&self, task: &Arc<TaskCore>) -> Result<Arc<KeySession>, Halt> {
        loop {
            let seen = self.plugged.load(Ordering::SeqCst);
            if let Some(session) = self.session() {
                task.bind(&session);
                return Ok(session);
            }
            task.set(KeyTaskState::PlugIn);
            self.lock().waiting.push(Arc::downgrade(task));
            task.wait_until(|| self.plugged.load(Ordering::SeqCst) != seen)?;
        }
    }

    /// Where a new task starts: the first stage that still needs doing.
    /// `check` runs what can be refused before any PIN or touch.
    fn initial(
        session: Option<&KeySession>,
        check: impl FnOnce(&KeySession) -> Result<(), KeyError>,
    ) -> KeyTaskState {
        let Some(session) = session else { return KeyTaskState::PlugIn };
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
            tries_left: (prompt.tries_left < 3).then_some(prompt.tries_left),
            forgot_after_background: prompt.forgot_after_background,
            last_error: None,
        }
    }

    /// Starts a task on the connected key's session, or at `PlugIn` if
    /// there is none. A task keeps the session it started with: if that
    /// key goes away, the task fails with `KeyRemoved`.
    fn start(
        self: &Arc<Self>,
        purpose: KeyPurpose,
        listener: Arc<dyn KeyTaskListener>,
        check: impl FnOnce(&KeySession) -> Result<(), KeyError>,
        body: impl FnOnce(&Arc<TaskCore>, &KeySession) -> Result<Option<KeyOutcome>, Halt> + Send + 'static,
    ) -> Arc<KeyTask> {
        let session = self.session();
        let initial = Self::initial(session.as_deref(), check);
        let port = self.clone();
        let body: Body = Box::new(move |task| {
            let session = match session {
                Some(session) => {
                    task.bind(&session);
                    session
                }
                None => port.wait_for_session(task)?,
            };
            body(task, &session)
        });
        KeyTask::start(purpose, listener, initial, body)
    }

    fn open_or_check(
        self: &Arc<Self>,
        purpose: KeyPurpose,
        store: Arc<PassStore>,
        entry: String,
        copies: Option<(Arc<CopyCache>, Duration)>,
        listener: Arc<dyn KeyTaskListener>,
    ) -> Arc<KeyTask> {
        let check_store = store.clone();
        let check_entry = entry.clone();
        let keep = matches!(purpose, KeyPurpose::OpenEntry { .. });
        self.start(
            purpose,
            listener,
            move |session| read_and_check(&check_store, &check_entry, session).map(drop),
            move |task, session| {
                let ciphertext = read_and_check(&store, &entry, session)?;
                let key = session.decryption_key()?;
                let plan = Plan::new(&ciphertext, key)?;
                let session_key = task.unlocked(session, |job| plan.decipher(job, task, key))?;
                if task.cancelled() {
                    // The late answer of a cancelled task: wiped on drop.
                    return Err(Halt::Fail(KeyError::Cancelled));
                }
                let plaintext = plan.finish(session_key)?;
                if std::str::from_utf8(&plaintext).is_err() {
                    return Err(Halt::Fail(KeyError::EntryUnreadable { detail: "it is not text".into() }));
                }
                if !keep {
                    return Ok(Some(KeyOutcome::KeyChecked));
                }
                if !task.begin_commit() {
                    return Err(Halt::Fail(KeyError::Cancelled));
                }
                // The entry opens even if keeping the copy fails: the
                // person has already touched the key.
                let face_id_until_ms = copies.and_then(|(copies, ttl)| {
                    copies.put(&entry, &plaintext, &content_id(&ciphertext), ttl).ok().flatten()
                });
                let mut plaintext = plaintext;
                let entry = OpenedEntry::from_plaintext(std::mem::take(&mut *plaintext))
                    .map_err(entry_unreadable)?;
                Ok(Some(KeyOutcome::Opened { entry, face_id_until_ms }))
            },
        )
    }
}

#[uniffi::export]
impl KeyPort {
    /// `clock` measures time in the background for the PIN limit (its
    /// monotonic clock).
    #[uniffi::constructor]
    pub fn new(clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            plugged: AtomicU64::new(0),
            state: Mutex::new(State { session: None, waiting: Vec::new() }),
        })
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
        let purpose = KeyPurpose::OpenEntry { entry: entry.clone() };
        let copies = copies.map(|copies| (copies, Duration::from_secs(ttl_secs.into())));
        self.open_or_check(purpose, store, entry, copies, listener)
    }

    /// Setup's key check: opens `entry` to prove the key works, and wipes
    /// the text at once. No copy is kept.
    pub fn check_key(self: Arc<Self>, store: Arc<PassStore>, entry: String, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        let purpose = KeyPurpose::CheckKey { entry: entry.clone() };
        self.open_or_check(purpose, store, entry, None, listener)
    }

    /// Unlocks the key for a git login. Once the PIN is verified the task
    /// shows `Working`; the Rust caller then takes `login_signer`, opens the
    /// connection, and calls `finish_login`.
    pub fn sign_in(self: Arc<Self>, login: LoginPurpose, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        self.start(
            KeyPurpose::SignIn { login },
            listener,
            |session| session.ssh_key().map(drop),
            |task, session| {
                session.ssh_key()?;
                task.unlocked(session, |_| Ok(()))?;
                task.set(KeyTaskState::Working { progress: None });
                task.mark_signer_ready();
                Ok(None)
            },
        )
    }
}

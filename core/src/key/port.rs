//! Where the key is plugged in.
//!
//! The app keeps one [`KeyPort`] for its lifetime. Plugging a key in makes a
//! [`KeySession`]; unplugging ends it and drops the PIN. A task started
//! with no key connected waits at `PlugIn` ([`KeyPort::wait_for_session`])
//! and goes on by itself when one is plugged in.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use super::enrolment::{self, KeyIdentity, KeyMismatch};
use super::keylog::Sink;
use super::session::{Halt, KeySession};
use super::task::TaskSide;
use super::{CardConnection, KeyError};
use crate::PawError;
use crate::clock::Clock;

struct State {
    session: Option<Arc<KeySession>>,
    /// Tasks at `PlugIn`.
    waiting: Vec<Weak<dyn TaskSide>>,
    /// The key the store was set up with. While set, no task sends the PIN
    /// to another card.
    enrolled: Option<KeyIdentity>,
    log: Option<Arc<Sink>>,
}

fn enrolment_check(expected: Option<&KeyIdentity>, session: &KeySession) -> Option<KeyMismatch> {
    expected.and_then(|expected| expected.check(&KeyIdentity::of(session.card_info())))
}

#[derive(uniffi::Object)]
pub struct KeyPort {
    uptime: Arc<dyn Clock>,
    /// Counts plug-ins, so a task at `PlugIn` cannot miss one.
    plugged: AtomicU64,
    state: Mutex<State>,
}

impl KeyPort {
    fn lock(&self) -> MutexGuard<'_, State> {
        // Plain values only.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Waits at `PlugIn` until a key is connected: the connected key's
    /// session.
    pub(crate) fn wait_for_session(&self, task: &Arc<dyn TaskSide>) -> Result<Arc<KeySession>, Halt> {
        loop {
            let seen = self.plugged.load(Ordering::SeqCst);
            if let Some(session) = self.session() {
                return Ok(session);
            }
            task.show_plug_in();
            self.lock().waiting.push(Arc::downgrade(task));
            task.wait_until(&mut || self.plugged.load(Ordering::SeqCst) != seen)?;
        }
    }

    pub(crate) fn refusal(expected: Option<&KeyIdentity>, session: &KeySession) -> Option<KeyError> {
        enrolment_check(expected, session).map(|mismatch| KeyError::DifferentKey { mismatch: Box::new(mismatch) })
    }

    /// How the plugged-in key differs from the enrolled one, or `None` if it
    /// is the enrolled key, no key is enrolled, or no key is plugged in.
    pub fn key_mismatch(&self) -> Option<KeyMismatch> {
        let session = self.session()?;
        enrolment_check(self.expected().as_ref(), &session)
    }

    pub(crate) fn key_log_dir(&self) -> Option<String> {
        self.lock().log.as_ref().map(|sink| sink.dir().to_string_lossy().into_owned())
    }

    /// The key tasks must be on: the enrolled one, if any.
    pub(crate) fn expected(&self) -> Option<KeyIdentity> {
        self.lock().enrolled.clone()
    }
}

#[uniffi::export]
impl KeyPort {
    /// `clock` measures time in the background for the PIN limit (its
    /// monotonic clock).
    #[uniffi::constructor]
    pub fn new(clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            uptime: clock,
            plugged: AtomicU64::new(0),
            state: Mutex::new(State { session: None, waiting: Vec::new(), enrolled: None, log: None }),
        })
    }

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

    /// Every process's key log lines in the folder this one writes to
    /// ([`Self::set_key_log`]), merged by time, oldest first; none if no
    /// log is set. See [`super::keylog::key_log`].
    pub fn key_log(&self, utc_offset_secs: i32) -> Vec<super::KeyLogLine> {
        self.key_log_dir().map(|dir| super::keylog::key_log(dir, utc_offset_secs)).unwrap_or_default()
    }

    /// [`Self::key_log`] as plain text, one line each: what Settings › Key
    /// diagnostics copies. It holds no secrets.
    pub fn key_log_text(&self, utc_offset_secs: i32) -> String {
        self.key_log_dir().map(|dir| super::keylog::key_log_text(dir, utc_offset_secs)).unwrap_or_default()
    }

    pub fn clear_key_log(&self) -> Result<(), PawError> {
        self.key_log_dir().map_or(Ok(()), super::keylog::clear_key_log)
    }

    /// Whether a key is plugged in and tasks may send it the PIN: it is the
    /// enrolled key, or none is enrolled.
    pub fn key_matches_enrolment(&self) -> bool {
        self.session().is_some() && self.key_mismatch().is_none()
    }
}

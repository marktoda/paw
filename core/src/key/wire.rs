//! The session's own transport wrapper. Every command a session sends goes
//! through it, the library's as well as paw's, so it is the one place that
//! knows:
//!
//! - when a command that may wait for a touch actually goes out, which is
//!   when the sheet may show `Touch` (design.md 3.5);
//! - the instruction and status word of the last exchange, so a failure
//!   the library reports as text can be classified from the status word;
//! - whether the reader failed, which ends the session.
//!
//! It records instruction bytes and status words only, never command or
//! response data, and writes each exchange to the session's key log
//! ([`super::keylog`]) once the card has answered.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use super::keylog::KeyLog;
use super::card::{CardConnection, ConnectionError};
use super::{KeyError, apdu};

/// Told when a command that may wait for a touch goes out, and when its
/// answer (or the reader's failure) comes back.
pub(crate) trait TouchHook: Send + Sync {
    fn sent(&self);
    fn answered(&self);
}

#[derive(Default)]
struct Record {
    failed: bool,
    /// INS and status word of the last command (a GET RESPONSE keeps the
    /// INS of the command it continues).
    last: Option<(u8, u16)>,
    touch_hook: Option<Arc<dyn TouchHook>>,
}

pub(crate) struct Wire {
    inner: Arc<dyn CardConnection>,
    record: Mutex<Record>,
    log: Arc<KeyLog>,
}

impl Wire {
    pub(crate) fn logged(inner: Arc<dyn CardConnection>, log: Arc<KeyLog>) -> Arc<Self> {
        Arc::new(Self { inner, record: Mutex::new(Record::default()), log })
    }

    pub(crate) fn log(&self) -> &Arc<KeyLog> {
        &self.log
    }

    fn record(&self) -> MutexGuard<'_, Record> {
        // Only plain values are written under this lock.
        self.record.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The reader failed once; nothing is sent after that.
    pub(crate) fn failed(&self) -> bool {
        self.record().failed
    }

    pub(crate) fn last(&self) -> Option<(u8, u16)> {
        self.record().last
    }

    /// Forgets the last exchange, so that [`Self::last`] afterwards is
    /// about a command sent since.
    pub(crate) fn clear_last(&self) {
        self.record().last = None;
    }

    /// Told about every command that may wait for a touch. Set for the
    /// length of one card job.
    pub(crate) fn set_touch_hook(&self, hook: Option<Arc<dyn TouchHook>>) {
        self.record().touch_hook = hook;
    }

    /// Sends a command whose answer fits in one response, and splits off
    /// the status word.
    pub(crate) fn send(&self, apdu: &[u8]) -> Result<(Vec<u8>, u16), KeyError> {
        let mut response = self.transmit(apdu.to_vec()).map_err(|_| KeyError::KeyRemoved)?;
        let Some(split) = response.len().checked_sub(2) else {
            return Err(KeyError::Reader { detail: "the reader returned no status word".into() });
        };
        let sw = u16::from_be_bytes([response[split], response[split + 1]]);
        response.truncate(split);
        Ok((response, sw))
    }
}

impl CardConnection for Wire {
    fn transmit(&self, command: Vec<u8>) -> Result<Vec<u8>, ConnectionError> {
        let hook = {
            let record = self.record();
            if record.failed {
                return Err(ConnectionError::Failed { message: "the reader failed earlier".into() });
            }
            apdu::waits_for_touch(&command).then(|| record.touch_hook.clone()).flatten()
        };
        if let Some(hook) = &hook {
            hook.sent();
        }
        let sent = Instant::now();
        let result = self.inner.transmit(command.clone());
        let round_trip = sent.elapsed();

        let sw = {
            let mut record = self.record();
            match &result {
                Err(_) => {
                    record.failed = true;
                    None
                }
                Ok(response) => {
                    let ins = match command.get(1) {
                        Some(&apdu::GET_RESPONSE) => record.last.map(|(ins, _)| ins),
                        other => other.copied(),
                    };
                    let sw = match response.as_slice() {
                        [.., hi, lo] => Some(u16::from_be_bytes([*hi, *lo])),
                        _ => None,
                    };
                    record.last = ins.zip(sw);
                    sw
                }
            }
        };
        // After the answer and outside the record's lock. The header only:
        // the fifth byte would be the length of the data, which for a
        // VERIFY is the PIN's.
        if let [cla, ins, p1, p2, ..] = command.as_slice() {
            self.log.command([*cla, *ins, *p1, *p2], apdu::has_data(&command), sw, round_trip, sent);
        }
        // After the log line, so the log reads command, then stage.
        if let Some(hook) = &hook {
            hook.answered();
        }
        result
    }
}

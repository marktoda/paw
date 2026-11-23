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
//! response data.

use std::sync::{Arc, Mutex, MutexGuard};

use super::KeyError;
use crate::ApduTransport;

/// PSO:DECIPHER (`2A 80 86`) and INTERNAL AUTHENTICATE (`88`) are the
/// commands a card holds until it is touched.
fn waits_for_touch(apdu: &[u8]) -> bool {
    match apdu {
        // The last (or only) part of a chain: CLA bit 0x10 clear.
        [cla, 0x2A, 0x80, 0x86, ..] | [cla, 0x88, ..] => cla & 0x10 == 0,
        _ => false,
    }
}

const GET_RESPONSE: u8 = 0xC0;

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
    inner: Arc<dyn ApduTransport>,
    record: Mutex<Record>,
}

impl Wire {
    pub(crate) fn new(inner: Arc<dyn ApduTransport>) -> Arc<Self> {
        Arc::new(Self { inner, record: Mutex::new(Record::default()) })
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
        let mut response = self.transmit(apdu).map_err(|_| KeyError::KeyRemoved)?;
        let Some(split) = response.len().checked_sub(2) else {
            return Err(KeyError::Reader { detail: "the reader returned no status word".into() });
        };
        let sw = u16::from_be_bytes([response[split], response[split + 1]]);
        response.truncate(split);
        Ok((response, sw))
    }
}

impl ApduTransport for Wire {
    fn transmit(&self, apdu: &[u8]) -> Result<Vec<u8>, String> {
        let hook = {
            let record = self.record();
            if record.failed {
                return Err("the reader failed earlier".into());
            }
            waits_for_touch(apdu).then(|| record.touch_hook.clone()).flatten()
        };
        if let Some(hook) = &hook {
            hook.sent();
        }
        let result = self.inner.transmit(apdu);
        if let Some(hook) = &hook {
            hook.answered();
        }

        let mut record = self.record();
        match &result {
            Err(_) => record.failed = true,
            Ok(response) => {
                let ins = match apdu.get(1) {
                    Some(&GET_RESPONSE) => record.last.map(|(ins, _)| ins),
                    other => other.copied(),
                };
                let sw = match response.as_slice() {
                    [.., hi, lo] => Some(u16::from_be_bytes([*hi, *lo])),
                    _ => None,
                };
                record.last = ins.zip(sw);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::waits_for_touch;

    #[test]
    fn only_the_last_part_of_a_key_operation_waits_for_a_touch() {
        assert!(waits_for_touch(&[0x00, 0x2A, 0x80, 0x86, 0x01, 0x00]));
        assert!(waits_for_touch(&[0x00, 0x88, 0x00, 0x00, 0x01, 0x00]));
        assert!(!waits_for_touch(&[0x10, 0x88, 0x00, 0x00, 0x01, 0x00]));
        // PSO:COMPUTE DIGITAL SIGNATURE is not used; neither is a short APDU.
        assert!(!waits_for_touch(&[0x00, 0x2A, 0x9E, 0x9A, 0x01, 0x00]));
        assert!(!waits_for_touch(&[0x00, 0x20, 0x00, 0x82]));
        assert!(!waits_for_touch(&[0x00]));
    }
}

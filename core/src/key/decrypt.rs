//! Opening an entry, split by what needs the card.
//!
//! [`Plan::new`] reads the entry and decides which recipient packets the
//! plugged-in key can unwrap, with no card exchange: an entry for another
//! key is refused here, before the PIN or a touch. [`Plan::decipher`] is the
//! card's part, one PSO:DECIPHER per candidate (one for an entry that names
//! this key, however many recipients it has). [`Plan::finish`] is the
//! symmetric decryption, after the card is free again.
//!
//! The same rules as [`crate::decrypt_entry`], which stays for the one-shot
//! functions Swift still calls.

use std::sync::Arc;

use openpgp_card::ocard::KeyType;
use openpgp_card_rpgp::CardSlot;
use pgp::composed::{Edata, Esk, Message};
use pgp::composed::PlainSessionKey;
use pgp::packet::{PublicKey, PublicKeyEncryptedSessionKey};
use pgp::types::{DecryptionKey, EskType, Password};
use zeroize::Zeroizing;

use super::session::{CardJob, Halt, card_error};
use super::status::{self, KeyOperation};
use super::task::TaskCore;
use super::touch::{self, TouchWatch};
use super::KeyError;
use crate::recipients;

const DECIPHER: u8 = 0x2A;

fn unreadable(detail: impl std::fmt::Display) -> KeyError {
    KeyError::EntryUnreadable { detail: detail.to_string() }
}

fn is_hidden(pkesk: &PublicKeyEncryptedSessionKey) -> bool {
    match pkesk {
        PublicKeyEncryptedSessionKey::V3 { id, .. } => id.is_wildcard(),
        PublicKeyEncryptedSessionKey::V6 { fingerprint, .. } => fingerprint.is_none(),
        _ => false,
    }
}

pub(crate) struct Plan<'a> {
    /// The one packet the card is asked to unwrap: the first addressed to
    /// this key, or if none is, the first hidden one (see
    /// [`crate::MAX_DECIPHERS`]).
    candidates: Vec<PublicKeyEncryptedSessionKey>,
    edata: Edata<'a>,
}

impl<'a> Plan<'a> {
    pub(crate) fn new(ciphertext: &'a [u8], key: &PublicKey) -> Result<Self, KeyError> {
        let message = Message::from_bytes(ciphertext).map_err(unreadable)?;
        let Message::Encrypted { esk, edata, .. } = message else {
            return Err(unreadable("it is not encrypted"));
        };
        let for_this_key = esk.into_iter().filter_map(|esk| match esk {
            Esk::PublicKeyEncryptedSessionKey(pkesk) if pkesk.match_identity(key) => Some(pkesk),
            _ => None,
        });
        let (hidden, named): (Vec<_>, Vec<_>) = for_this_key.partition(is_hidden);
        let mut candidates = if named.is_empty() { hidden } else { named };
        candidates.truncate(crate::MAX_DECIPHERS);
        if candidates.is_empty() {
            let recipients = recipients::entry_recipients(ciphertext)
                .map(|found| found.keys.iter().map(recipients::Recipient::describe).collect())
                .unwrap_or_default();
            return Err(KeyError::NotForThisKey { recipients });
        }
        Ok(Self { candidates, edata })
    }

    /// Asks the card to unwrap the session key. Shows `Touch` as each
    /// command goes out, if the slot's policy can ask for one.
    pub(crate) fn decipher(
        &self,
        job: &mut CardJob<'_>,
        task: &Arc<TaskCore>,
        key: &PublicKey,
    ) -> Result<PlainSessionKey, Halt> {
        let session = job.session();
        let wire = session.wire().clone();
        let policy = session.card_info().decryption_touch;
        let touch_possible = touch::possible(policy);
        let mut tx = job.card.transaction().map_err(|e| card_error(&wire, "starting to decrypt", e))?;
        let slot = CardSlot::with_public_key(&mut tx, KeyType::Decryption, key.clone(), &|| {})
            .map_err(|e| card_error(&wire, "reading the decryption key", e))?;
        let _watch = TouchWatch::arm(&wire, task, policy);

        let mut failure = KeyError::DecryptFailed { detail: "the key did not answer".into() };
        for pkesk in &self.candidates {
            if task.cancelled() {
                return Err(Halt::Fail(KeyError::Cancelled));
            }
            let version = match pkesk {
                PublicKeyEncryptedSessionKey::V6 { .. } => EskType::V6,
                _ => EskType::V3_4,
            };
            wire.clear_last();
            let unwrapped = pkesk
                .values()
                .and_then(|values| slot.decrypt(&Password::empty(), values, version))
                .and_then(|inner| inner);
            let error = match unwrapped {
                Ok(session_key) => return Ok(session_key),
                Err(e) => e,
            };
            if wire.failed() {
                return Err(Halt::Fail(KeyError::KeyRemoved));
            }
            match wire.last() {
                Some((DECIPHER, sw)) => match status::key_operation(sw, touch_possible) {
                    // The card answered; what it gave did not unwrap this
                    // packet. Try the next hidden one.
                    KeyOperation::Done => failure = KeyError::DecryptFailed { detail: error.to_string() },
                    KeyOperation::TouchTimeout => return Err(Halt::Fail(KeyError::TouchTimeout)),
                    KeyOperation::Refused(sw) => return Err(Halt::Fail(status::unexpected("decrypting", sw))),
                },
                Some((_, sw)) if sw != status::OK => {
                    return Err(Halt::Fail(status::unexpected("decrypting", sw)));
                }
                _ => failure = KeyError::DecryptFailed { detail: error.to_string() },
            }
        }
        Err(Halt::Fail(failure))
    }

    /// Decrypts the entry with the unwrapped session key. No card.
    pub(crate) fn finish(mut self, session_key: PlainSessionKey) -> Result<Zeroizing<Vec<u8>>, KeyError> {
        let failed = |e: pgp::errors::Error| KeyError::DecryptFailed { detail: e.to_string() };
        self.edata.decrypt(&session_key).map_err(failed)?;
        drop(session_key);
        let mut decrypted = Message::from_bytes(self.edata).map_err(failed)?;
        if decrypted.is_compressed() {
            decrypted = decrypted.decompress().map_err(failed)?;
        }
        let mut plaintext = Zeroizing::new(Vec::new());
        crate::read_capped(&mut decrypted, &mut plaintext)
            .map_err(|detail| KeyError::DecryptFailed { detail })?;
        Ok(plaintext)
    }
}

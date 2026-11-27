//! Opening an entry, split by what needs the card.

use std::io::Read;
use std::sync::Arc;

use openpgp_card::ocard::KeyType;
use openpgp_card_rpgp::CardSlot;
use pgp::composed::{Edata, Esk, Message};
use pgp::composed::PlainSessionKey;
use pgp::packet::{PublicKey, PublicKeyEncryptedSessionKey};
use pgp::types::{DecryptionKey, EskType, Password};
use zeroize::Zeroizing;

use super::session::{CardJob, Halt, card_error};
use super::task::TaskSide;
use super::{KeyError, KeySlot};
use crate::fingerprint::Fingerprint;
use crate::recipients::{self, Access};

use crate::entry::MAX_PLAINTEXT_BYTES;

fn read_capped(from: &mut impl Read, out: &mut Vec<u8>) -> Result<(), String> {
    Read::take(from, MAX_PLAINTEXT_BYTES + 1)
        .read_to_end(out)
        .map_err(|e| e.to_string())?;
    if out.len() as u64 > MAX_PLAINTEXT_BYTES {
        use zeroize::Zeroize;
        out.zeroize();
        return Err("it holds more text than any password entry should".into());
    }
    Ok(())
}

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
    /// The one recipient packet the card is asked to unwrap: the first that
    /// names the card's decryption key, or, if none names it, the first
    /// hidden (wildcard) packet. Each try can cost a touch, so a crafted
    /// entry with many packets for this key cannot make the person touch the
    /// key again and again. An entry whose only packets are hidden, for
    /// several recipients, opens only if this key's packet is the first:
    /// `gpg --throw-keyids` with several recipients is rare in password
    /// stores, and a failed open says the key could not decrypt it.
    packet: PublicKeyEncryptedSessionKey,
    edata: Edata<'a>,
}

impl<'a> Plan<'a> {
    /// Refuses, before the PIN or a touch, an entry the key cannot open,
    /// by the rule Home's badge uses ([`recipients::access`]), so the two
    /// never disagree.
    pub(crate) fn new(ciphertext: &'a [u8], key: &PublicKey) -> Result<Self, KeyError> {
        let not_for_this_key = |keys: &[recipients::Recipient]| KeyError::NotForThisKey { recipients: keys.to_vec() };
        match recipients::key_access(ciphertext, Some(&Fingerprint::of(key))) {
            Access::CanOpen | Access::MightOpen => {}
            Access::NotForThisKey { recipients } => return Err(not_for_this_key(&recipients)),
            Access::Unreadable { reason } => return Err(unreadable(reason)),
        }
        let message = Message::from_bytes(ciphertext).map_err(unreadable)?;
        let Message::Encrypted { esk, edata, .. } = message else {
            return Err(unreadable("it is not encrypted"));
        };
        let for_this_key = esk.into_iter().filter_map(|esk| match esk {
            Esk::PublicKeyEncryptedSessionKey(pkesk) if pkesk.match_identity(key) => Some(pkesk),
            _ => None,
        });
        let (hidden, named): (Vec<_>, Vec<_>) = for_this_key.partition(is_hidden);
        let Some(packet) = named.into_iter().next().or_else(|| hidden.into_iter().next()) else {
            // The headers name this key, yet the OpenPGP library finds no
            // packet for it: the file is not what its headers say.
            return Err(unreadable("its recipient packets do not match its headers"));
        };
        Ok(Self { packet, edata })
    }

    /// Asks the card to unwrap the session key. Shows `Touch` as the
    /// command goes out, if the slot's policy can ask for one.
    pub(crate) fn decipher(
        &self,
        job: &mut CardJob<'_>,
        task: &Arc<dyn TaskSide>,
        key: &PublicKey,
    ) -> Result<PlainSessionKey, Halt> {
        let session = job.session();
        let wire = session.wire().clone();
        let mut tx = job.card.transaction().map_err(|e| card_error(&wire, "starting to decrypt", e))?;
        let slot = CardSlot::with_public_key(&mut tx, KeyType::Decryption, key.clone(), &|| {})
            .map_err(|e| card_error(&wire, "reading the decryption key", e))?;
        if task.cancelled() {
            return Err(Halt::Cancelled);
        }
        let version = match self.packet {
            PublicKeyEncryptedSessionKey::V6 { .. } => EskType::V6,
            _ => EskType::V3_4,
        };
        let unwrapped = session.touch_op(task, KeySlot::Decryption, || {
            self.packet.values().and_then(|values| slot.decrypt(&Password::empty(), values, version)).and_then(|inner| inner)
        })?;
        // The card answered; what it gave did not unwrap the packet.
        unwrapped.map_err(|error| Halt::Fail(KeyError::DecryptFailed { detail: error.to_string() }))
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
        read_capped(&mut decrypted, &mut plaintext)
            .map_err(|detail| KeyError::DecryptFailed { detail })?;
        Ok(plaintext)
    }
}

//! Who can open an entry, read from the file without decrypting it.
//!
//! An encrypted OpenPGP message starts with one small packet per recipient,
//! in the clear, naming the key that can unwrap the session key. Reading
//! those tells paw whether the plugged-in key can open the entry before it
//! asks for a PIN or a touch. That matters in practice: a store that has
//! lived through a key change holds entries only the old key can open.
//!
//! The walker is written by hand rather than on `pgp`'s packet parser
//! because it must stop at the encrypted data, never allocate from a length
//! found in the file, and treat every byte as hostile. It reads headers and
//! the first few bytes of each recipient packet, nothing else.

use crate::PawError;
use crate::fingerprint::{Fingerprint, hex};

/// One key an entry is encrypted for, as far as the file says.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Recipient {
    /// The 64-bit ID of a (sub)key, 16 upper-case hex digits. What GnuPG
    /// writes for every v4 key.
    KeyId { id: String },
    /// A full fingerprint in upper-case hex (v6 recipient packets).
    Fingerprint { fingerprint: String },
    /// The recipient was hidden on purpose (`gpg --throw-keyids`): only
    /// trying a key tells whether it fits.
    Hidden,
}

impl Recipient {
    /// Whether this packet names the key with `fingerprint`: by that
    /// fingerprint, or by its key ID. A hidden recipient names no key.
    /// The one rule for "is this entry for that key": Home's badge, the
    /// key open, held-back changes, setup's binding and the recipients
    /// screen all ask it.
    pub(crate) fn names(&self, fingerprint: &Fingerprint) -> bool {
        match self {
            Self::KeyId { id } => fingerprint.key_id() == id,
            Self::Fingerprint { fingerprint: named } => named == fingerprint.as_str(),
            Self::Hidden => false,
        }
    }
}

/// Everything an entry's clear-text headers say about who can open it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EntryRecipients {
    /// In file order. Empty when the entry is locked with a passphrase only.
    pub keys: Vec<Recipient>,
    /// The entry can (also) be opened with a passphrase (`gpg -c`).
    pub passphrase: bool,
}

/// Whether the plugged-in key can open an entry.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Access {
    /// The entry names this key.
    CanOpen,
    /// The entry hides its recipients and names no other key that settles
    /// it: only a try will tell.
    MightOpen,
    /// The entry names keys, and none is this one. No PIN or touch can help.
    NotForThisKey { recipients: Vec<Recipient> },
    /// Not something a key can open: not an OpenPGP message, damaged, or
    /// locked with a passphrase only.
    Unreadable { reason: String },
}

const TAG_PKESK: u8 = 1;
const TAG_SKESK: u8 = 3;
const TAG_SED: u8 = 9;
const TAG_MARKER: u8 = 10;
const TAG_SEIPD: u8 = 18;
/// GnuPG's own AEAD packet, written when both sides advertise OCB.
const TAG_GNUPG_AEAD: u8 = 20;

enum Length {
    Exactly(usize),
    /// Partial body lengths or "until the end of the file": the body is
    /// streamed. Only data packets may do this.
    Streamed,
}

fn truncated() -> PawError {
    PawError::NotAnEntry("the file ends in the middle of a packet header".into())
}

fn header(bytes: &[u8], at: usize) -> Result<(u8, Length, usize), PawError> {
    let byte = |i: usize| bytes.get(i).copied().ok_or_else(truncated);
    let number = |from: usize, len: usize| -> Result<usize, PawError> {
        let digits = bytes.get(from..from + len).ok_or_else(truncated)?;
        Ok(digits.iter().fold(0usize, |n, b| (n << 8) | usize::from(*b)))
    };

    let first = byte(at)?;
    if first & 0x80 == 0 {
        return Err(PawError::NotAnEntry("it is not OpenPGP data".into()));
    }
    if first & 0x40 != 0 {
        // New format: the tag is the low six bits, the length follows in
        // one, two or five bytes, or as a power-of-two partial length.
        let tag = first & 0x3F;
        let (length, body) = match byte(at + 1)? {
            n @ 0..=191 => (Length::Exactly(n.into()), at + 2),
            n @ 192..=223 => {
                let low = usize::from(byte(at + 2)?);
                (Length::Exactly(((usize::from(n) - 192) << 8) + low + 192), at + 3)
            }
            255 => (Length::Exactly(number(at + 2, 4)?), at + 6),
            _ => (Length::Streamed, at + 1),
        };
        Ok((tag, length, body))
    } else {
        // Old format: tag in bits 5..2, length type in the low two bits.
        let tag = (first >> 2) & 0x0F;
        let (length, body) = match first & 0x03 {
            0 => (Length::Exactly(number(at + 1, 1)?), at + 2),
            1 => (Length::Exactly(number(at + 1, 2)?), at + 3),
            2 => (Length::Exactly(number(at + 1, 4)?), at + 5),
            _ => (Length::Streamed, at + 1),
        };
        Ok((tag, length, body))
    }
}

fn recipient(body: &[u8]) -> Result<Recipient, PawError> {
    let damaged = || PawError::NotAnEntry("a recipient packet is too short".into());
    match body.first() {
        // Version 3: an eight-byte key ID, all zero for a hidden recipient.
        Some(3) => {
            let id = body.get(1..9).ok_or_else(damaged)?;
            if id.iter().all(|b| *b == 0) {
                Ok(Recipient::Hidden)
            } else {
                Ok(Recipient::KeyId { id: hex(id) })
            }
        }
        // Version 6: a length, then the key's version and its fingerprint.
        // A length of zero hides the recipient.
        Some(6) => match usize::from(*body.get(1).ok_or_else(damaged)?) {
            0 => Ok(Recipient::Hidden),
            len => {
                let named = body.get(2..2 + len).ok_or_else(damaged)?;
                let fingerprint = named.get(1..).filter(|f| !f.is_empty()).ok_or_else(damaged)?;
                Ok(Recipient::Fingerprint { fingerprint: hex(fingerprint) })
            }
        },
        Some(version) => Err(PawError::NotAnEntry(format!(
            "it uses recipient packets of version {version}, which paw does not know"
        ))),
        None => Err(damaged()),
    }
}

/// Reads who `bytes` (a whole `.gpg` file, or at least its start up to the
/// encrypted data) is encrypted for. Nothing is decrypted.
pub fn entry_recipients(bytes: &[u8]) -> Result<EntryRecipients, PawError> {
    if bytes.is_empty() {
        return Err(PawError::NotAnEntry("the file is empty".into()));
    }
    if bytes.starts_with(b"-----BEGIN PGP") {
        return Err(PawError::NotAnEntry(
            "it is ASCII-armoured, and paw reads binary entries only".into(),
        ));
    }

    let mut found = EntryRecipients { keys: Vec::new(), passphrase: false };
    let mut at = 0;
    loop {
        if at >= bytes.len() {
            return Err(PawError::NotAnEntry("it holds no encrypted data".into()));
        }
        let (tag, length, start) = header(bytes, at)?;
        match tag {
            TAG_SED | TAG_SEIPD | TAG_GNUPG_AEAD => {
                if found.keys.is_empty() && !found.passphrase {
                    return Err(PawError::NotAnEntry("it names no recipient".into()));
                }
                return Ok(found);
            }
            TAG_PKESK | TAG_SKESK | TAG_MARKER => {
                let Length::Exactly(len) = length else {
                    return Err(PawError::NotAnEntry("a recipient packet has no length".into()));
                };
                let end = start.checked_add(len).filter(|end| *end <= bytes.len());
                let body = end.and_then(|end| bytes.get(start..end)).ok_or_else(|| {
                    PawError::NotAnEntry("the file ends in the middle of a recipient packet".into())
                })?;
                match tag {
                    TAG_PKESK => found.keys.push(recipient(body)?),
                    TAG_SKESK => found.passphrase = true,
                    _ => {}
                }
                at = start + len;
            }
            other => {
                return Err(PawError::NotAnEntry(format!(
                    "it is OpenPGP data but not an encrypted message (packet type {other})"
                )));
            }
        }
    }
}

pub(crate) fn names_key(recipients: &[Recipient], fingerprint: &Fingerprint) -> bool {
    recipients.iter().any(|recipient| recipient.names(fingerprint))
}

/// Decides whether the key whose decryption (sub)key has `card` can open
/// an entry with these recipients; `None` names no key.
///
/// The card reports the fingerprint of the decryption subkey, which is also
/// what a recipient packet names, so no certificate is needed to compare.
fn access_for(recipients: &EntryRecipients, card: Option<&Fingerprint>) -> Access {
    if recipients.keys.is_empty() {
        return Access::Unreadable {
            reason: "it is locked with a passphrase, not for a key".into(),
        };
    }
    if card.is_some_and(|card| names_key(&recipients.keys, card)) {
        Access::CanOpen
    } else if recipients.keys.contains(&Recipient::Hidden) {
        Access::MightOpen
    } else {
        Access::NotForThisKey { recipients: recipients.keys.clone() }
    }
}

/// Whether the key whose decryption fingerprint is `card` can open the
/// entry whose bytes are `bytes`.
pub(crate) fn key_access(bytes: &[u8], card: Option<&Fingerprint>) -> Access {
    match entry_recipients(bytes) {
        Ok(recipients) => access_for(&recipients, card),
        Err(PawError::NotAnEntry(reason)) => Access::Unreadable { reason },
        Err(other) => Access::Unreadable { reason: other.to_string() },
    }
}

/// [`key_access`] for a fingerprint given as text, as the card reports it
/// or a person pastes it. Text that is no fingerprint names no key.
pub fn access(bytes: &[u8], card_fingerprint: &str) -> Access {
    key_access(bytes, Fingerprint::parse(card_fingerprint).as_ref())
}


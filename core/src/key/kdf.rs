//! The card's PIN KDF (OpenPGP card 3.4, the KDF data object `F9`).
//!
//! A card with KDF turned on expects VERIFY to carry a hash of the PIN,
//! not the PIN: an iterated and salted S2K (RFC 4880 3.7.1.3) with SHA-256
//! or SHA-512. Sent the raw PIN, it counts a wrong try. The session reads
//! the object when the key is plugged in and derives what it sends from it;
//! a setting paw does not know is refused before any VERIFY.

use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

use super::KeyError;
use super::wire::Wire;

/// How the card wants the PIN sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PinKdf {
    /// As typed: no KDF data object, or its algorithm is "none".
    Plain,
    /// Iterated and salted S2K, hashing `count` bytes of salt and PIN.
    IterSalted { hash: u8, count: u32, salt: Vec<u8> },
    /// A KDF paw does not support: no VERIFY may be sent.
    Unsupported(String),
}

const SHA256: u8 = 0x08;
const SHA512: u8 = 0x0A;

/// One BER-TLV element: tag (one or two bytes), value, and what follows.
fn next_tlv(bytes: &[u8]) -> Option<(u16, &[u8], &[u8])> {
    let (&first, rest) = bytes.split_first()?;
    let (tag, rest) = if first & 0x1F == 0x1F {
        let (&second, rest) = rest.split_first()?;
        (u16::from_be_bytes([first, second]), rest)
    } else {
        (u16::from(first), rest)
    };
    let (&length, rest) = rest.split_first()?;
    let (length, rest) = match length {
        0x81 => {
            let (&n, rest) = rest.split_first()?;
            (usize::from(n), rest)
        }
        0x82 => {
            let [hi, lo, rest @ ..] = rest else { return None };
            (usize::from(u16::from_be_bytes([*hi, *lo])), rest)
        }
        n if n < 0x80 => (usize::from(n), rest),
        _ => return None,
    };
    let value = rest.get(..length)?;
    Some((tag, value, &rest[length..]))
}

impl PinKdf {
    /// Reads the card's KDF data object. A card that has none (it answers
    /// GET DATA with an error) takes the PIN as typed.
    pub(crate) fn read(wire: &Wire) -> Result<Self, KeyError> {
        let (body, sw) = wire.send(&[0x00, 0xCA, 0x00, 0xF9, 0x00])?;
        if sw != super::status::OK {
            return Ok(Self::Plain);
        }
        Ok(Self::parse(&body))
    }

    pub(crate) fn parse(body: &[u8]) -> Self {
        // Some cards wrap the contents in the `F9` tag itself.
        let mut fields = match next_tlv(body) {
            Some((0xF9, inner, _)) => inner,
            _ => body,
        };
        let (mut algorithm, mut hash, mut count, mut salt) = (None, None, None, None);
        while !fields.is_empty() {
            let Some((tag, value, rest)) = next_tlv(fields) else {
                return Self::Unsupported("the key's KDF setting cannot be read".into());
            };
            match (tag, value) {
                (0x81, [a]) => algorithm = Some(*a),
                (0x82, [h]) => hash = Some(*h),
                (0x83, [a, b, c, d]) => count = Some(u32::from_be_bytes([*a, *b, *c, *d])),
                (0x84, s) => salt = Some(s.to_vec()),
                _ => {}
            }
            fields = rest;
        }
        match (algorithm, hash, count, salt) {
            (None | Some(0), ..) => Self::Plain,
            (Some(3), Some(hash @ (SHA256 | SHA512)), Some(count), Some(salt)) => Self::IterSalted { hash, count, salt },
            (Some(3), ..) => Self::Unsupported("the key's PIN KDF uses a hash or setting paw does not support".into()),
            (Some(other), ..) => Self::Unsupported(format!("the key's PIN KDF (algorithm {other}) is not supported")),
        }
    }

    /// What VERIFY carries for `pin`.
    pub(crate) fn derive(&self, pin: &[u8]) -> Result<Zeroizing<Vec<u8>>, KeyError> {
        match self {
            Self::Plain => Ok(Zeroizing::new(pin.to_vec())),
            Self::IterSalted { hash, count, salt } => Ok(match *hash {
                SHA256 => iter_salted::<Sha256>(pin, salt, *count),
                _ => iter_salted::<Sha512>(pin, salt, *count),
            }),
            Self::Unsupported(why) => Err(unsupported(why)),
        }
    }
}

/// The refusal for a card whose KDF paw cannot follow. No VERIFY was sent.
pub(crate) fn unsupported(why: &str) -> KeyError {
    KeyError::Reader { detail: format!("{why}; paw has not sent it the PIN, so no try was used") }
}

/// RFC 4880 3.7.1.3: `salt || pin` repeated until `count` bytes are hashed,
/// and at least once whole.
fn iter_salted<D: Digest>(pin: &[u8], salt: &[u8], count: u32) -> Zeroizing<Vec<u8>> {
    let mut hasher = D::new();
    let unit = salt.len() + pin.len();
    let mut left = (count as usize).max(unit);
    while left >= unit {
        hasher.update(salt);
        hasher.update(pin);
        left -= unit;
    }
    let tail_salt = left.min(salt.len());
    hasher.update(&salt[..tail_salt]);
    hasher.update(&pin[..left - tail_salt]);
    Zeroizing::new(hasher.finalize().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example of the OpenPGP card 3.4.1 specification, page 20.
    #[test]
    fn derives_the_specifications_example() {
        let kdf = PinKdf::IterSalted { hash: SHA256, count: 100_000, salt: b"01234567".to_vec() };
        let derived = kdf.derive(b"123456").unwrap();
        assert_eq!(
            derived.as_slice(),
            [
                0x77, 0x37, 0x84, 0xA6, 0x02, 0xB6, 0xC8, 0x1E, 0x3F, 0x09, 0x2F, 0x4D, 0x7D, 0x00, 0xE1, 0x7C,
                0xC8, 0x22, 0xD8, 0x8F, 0x73, 0x60, 0xFC, 0xF2, 0xD2, 0xEF, 0x2D, 0x9D, 0x90, 0x1F, 0x44, 0xB6
            ]
        );
    }

    #[test]
    fn reads_the_data_object() {
        let object = [
            0x81, 0x01, 0x03, 0x82, 0x01, 0x08, 0x83, 0x04, 0x00, 0x01, 0x86, 0xA0, 0x84, 0x02, 0xAA, 0xBB,
        ];
        let expected = PinKdf::IterSalted { hash: SHA256, count: 100_000, salt: vec![0xAA, 0xBB] };
        assert_eq!(PinKdf::parse(&object), expected);
        let mut wrapped = vec![0xF9, object.len() as u8];
        wrapped.extend(object);
        assert_eq!(PinKdf::parse(&wrapped), expected);
        assert_eq!(PinKdf::parse(&[0x81, 0x01, 0x00]), PinKdf::Plain);
        assert_eq!(PinKdf::parse(&[]), PinKdf::Plain);
        assert!(matches!(PinKdf::parse(&[0x81, 0x01, 0x03, 0x82, 0x01, 0x09]), PinKdf::Unsupported(_)));
        assert!(matches!(PinKdf::parse(&[0x81, 0x01, 0x07]), PinKdf::Unsupported(_)));
        assert!(matches!(PinKdf::parse(&[0x81, 0x05]), PinKdf::Unsupported(_)));
    }
}

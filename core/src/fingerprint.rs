//! An OpenPGP key's fingerprint, in the one form the core compares: upper-
//! case hex, 40 digits for a v4 key, 64 for a v6 one.
//!
//! It is made where a fingerprint comes in (the card, a `.gpg-id` line, a
//! file in the state directory, a key) and compared as it is from then on.
//! Swift sees it as text: a `String` it hands back is read again, and one
//! that is no fingerprint is refused.

use std::fmt;

/// See the module documentation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// `text` as a fingerprint, the way a card, a person or a `.gpg-id`
    /// writes one: spaces and a leading `0x` dropped, either case. `None`
    /// unless 40 or 64 hex digits remain.
    pub fn parse(text: &str) -> Option<Self> {
        let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let digits = compact.strip_prefix("0x").or_else(|| compact.strip_prefix("0X")).unwrap_or(&compact);
        (matches!(digits.len(), 40 | 64) && digits.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| Self(digits.to_ascii_uppercase()))
    }

    /// The fingerprint of `key`. Every key paw handles is v4 or v6, the
    /// only versions a card or a `.gpg-id` fingerprint line can name.
    pub(crate) fn of(key: &impl pgp::types::KeyDetails) -> Self {
        Self::from_bytes(key.fingerprint().as_bytes())
    }

    /// A fingerprint as raw bytes, as a key or a card gives it.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        Self(hex(bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The 64-bit key ID a recipient packet names this key by: the low 64
    /// bits of a v4 fingerprint, the high 64 bits of a v6 one.
    pub fn key_id(&self) -> &str {
        if self.0.len() == 64 { &self.0[..16] } else { &self.0[self.0.len() - 16..] }
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

uniffi::custom_type!(Fingerprint, String, {
    lower: |fingerprint| fingerprint.0,
    try_lift: |text| Fingerprint::parse(&text).ok_or_else(|| uniffi::deps::anyhow::anyhow!("not a fingerprint")),
});

/// `bytes` as upper-case hex, the way fingerprints and key IDs are written.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// `text` in groups of four characters, as people read a fingerprint out:
/// "9A1C 03F2 …". Spaces in `text` are dropped first; the characters are
/// kept as they are (an SSH fingerprint is base64).
#[uniffi::export]
pub fn fingerprint_groups(text: String) -> Vec<String> {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    chars.chunks(4).map(|group| group.iter().collect()).collect()
}

/// How a person tells keys apart by eye: the last eight hex digits of
/// `fingerprint`, upper case, in two groups ("2C11", "41DE"). Fewer if it
/// has fewer digits.
#[uniffi::export]
pub fn fingerprint_tail(fingerprint: String) -> Vec<String> {
    let hex: String = fingerprint.chars().filter(char::is_ascii_hexdigit).map(|c| c.to_ascii_uppercase()).collect();
    let tail = &hex[hex.len().saturating_sub(8)..];
    let (first, last) = tail.split_at(tail.len().min(4));
    [first, last].into_iter().filter(|part| !part.is_empty()).map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::{fingerprint_groups, fingerprint_tail};

    #[test]
    fn fingerprints_read_in_groups_of_four_and_end_in_two() {
        assert_eq!(fingerprint_groups("AAAA BBBB CCCC D".into()), ["AAAA", "BBBB", "CCCC", "D"]);
        assert_eq!(fingerprint_groups("uNiVztFv8Wq2".into()), ["uNiV", "ztFv", "8Wq2"]);
        assert_eq!(fingerprint_tail("0x0123456789abcdef2c1141de".into()), ["2C11", "41DE"]);
        assert_eq!(fingerprint_tail("ABCDE".into()), ["ABCD", "E"]);
    }
}

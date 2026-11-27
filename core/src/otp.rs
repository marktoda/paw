//! One-time codes (TOTP, RFC 6238) from an entry's `otpauth://` line, read
//! and written the way pass-otp does.
//!
//! The secret is key material, as sensitive as a password: it is held in
//! [`Zeroizing`] memory, has no `Display`, is left out of `Debug`, and never
//! appears in an [`OtpProblem`]. Only the code it makes leaves this module.

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use zeroize::{Zeroize, Zeroizing};

const SCHEME: &str = "otpauth://";
/// Longest period accepted, in seconds. Real ones are 30 or 60.
const MAX_PERIOD: u32 = 300;
/// The settings of an address that names none, and of a bare setup key.
const DEFAULT_ALGORITHM: Algorithm = Algorithm::Sha1;
const DEFAULT_DIGITS: u32 = 6;
const DEFAULT_PERIOD: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Algorithm {
    Sha1,
    Sha256,
    Sha512,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtpProblem {
    NotAnAddress,
    /// `otpauth://hotp`: codes counted by use, not by time.
    CounterBased,
    NoSecret,
    BadSecret,
    UnknownAlgorithm,
    BadDigits,
    BadPeriod,
    /// A label pass-otp can't read: more than one `:`.
    BadLabel,
}

impl OtpProblem {
    /// A short sentence for the person. Never contains the secret.
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotAnAddress => "This isn't a setup key or an otpauth:// address.",
            Self::CounterBased => "Counter-based codes aren't supported.",
            Self::NoSecret => "The address has no setup key.",
            Self::BadSecret => "The setup key isn't valid.",
            Self::UnknownAlgorithm => "It uses an algorithm paw doesn't support.",
            Self::BadDigits => "Codes must have 6 to 8 digits.",
            Self::BadPeriod => "The code's time step isn't valid.",
            Self::BadLabel => "The address's name has more than one colon.",
        }
    }
}

pub struct Totp {
    secret: Zeroizing<Vec<u8>>,
    algorithm: Algorithm,
    digits: u32,
    period: u32,
}

impl std::fmt::Debug for Totp {
    /// The settings, never the secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Totp")
            .field("secret", &"<withheld>")
            .field("algorithm", &self.algorithm)
            .field("digits", &self.digits)
            .field("period", &self.period)
            .finish()
    }
}

pub(crate) fn is_address(text: &str) -> bool {
    text.get(..SCHEME.len()).is_some_and(|start| start.eq_ignore_ascii_case(SCHEME))
}

/// An address after its scheme, split where its type (`totp`) ends.
fn split_type(rest: &str) -> (&str, &str) {
    rest.split_at(rest.find(['/', '?']).unwrap_or(rest.len()))
}

/// What a setup key may hold between its symbols, as keys are shown and
/// typed: spaces and dashes.
fn spacing(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'-')
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// `%XX` decoding of a query value. A stray `%` stays as it is.
fn percent_decode(value: &str) -> Zeroizing<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut out = Zeroizing::new(Vec::with_capacity(bytes.len()));
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(high), Some(low)) = (bytes.get(i + 1).and_then(|b| hex(*b)), bytes.get(i + 2).and_then(|b| hex(*b)))
        {
            out.push(high << 4 | low);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// Decodes a base32 setup key (RFC 4648). Tolerates lowercase, spaces,
/// dashes and missing or present `=` padding, as keys are shown and typed.
fn base32(text: &[u8]) -> Result<Zeroizing<Vec<u8>>, OtpProblem> {
    let mut out = Zeroizing::new(Vec::with_capacity(text.len() * 5 / 8 + 1));
    let mut buffer: u64 = 0;
    let mut bits = 0;
    let mut symbols = 0usize;
    let mut padding = false;
    for &byte in text {
        let value = match byte.to_ascii_uppercase() {
            _ if spacing(byte) => continue,
            b'=' => {
                padding = true;
                continue;
            }
            _ if padding => return Err(OtpProblem::BadSecret),
            upper @ b'A'..=b'Z' => upper - b'A',
            digit @ b'2'..=b'7' => digit - b'2' + 26,
            _ => return Err(OtpProblem::BadSecret),
        };
        buffer = (buffer << 5) | u64::from(value);
        bits += 5;
        symbols += 1;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    buffer.zeroize();
    // A final group of 1, 3 or 6 symbols cannot come from whole bytes: a
    // character was lost or added.
    if symbols == 0 || matches!(symbols % 8, 1 | 3 | 6) {
        return Err(OtpProblem::BadSecret);
    }
    Ok(out)
}

impl Totp {
    /// Reads an `otpauth://totp/...` address. `secret` is required;
    /// `algorithm` (SHA1, SHA256, SHA512), `digits` (6 to 8) and `period`
    /// (seconds) default to SHA1, 6 and 30. Other parameters are ignored.
    pub fn from_address(address: &str) -> Result<Self, OtpProblem> {
        let address = address.trim();
        if !is_address(address) || address.contains(['\n', '\r']) {
            return Err(OtpProblem::NotAnAddress);
        }
        let (kind, rest) = split_type(&address[SCHEME.len()..]);
        match kind {
            kind if kind.eq_ignore_ascii_case("totp") => {}
            kind if kind.eq_ignore_ascii_case("hotp") => return Err(OtpProblem::CounterBased),
            _ => return Err(OtpProblem::NotAnAddress),
        }
        let query = rest.split_once('?').map_or("", |(_, query)| query);
        let mut secret = None;
        let mut algorithm = None;
        let mut digits = None;
        let mut period = None;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let slot = match key.to_ascii_lowercase().as_str() {
                "secret" => &mut secret,
                "algorithm" => &mut algorithm,
                "digits" => &mut digits,
                "period" => &mut period,
                _ => continue,
            };
            if slot.is_none() {
                *slot = Some(percent_decode(value));
            }
        }

        let secret = match secret {
            Some(secret) if !secret.iter().all(|b| b.is_ascii_whitespace() || *b == b'=') => base32(&secret)?,
            _ => return Err(OtpProblem::NoSecret),
        };
        let text = |value: &[u8]| String::from_utf8_lossy(value).trim().to_ascii_uppercase();
        let algorithm = match algorithm.as_deref().map(|value| text(value)).as_deref() {
            None => DEFAULT_ALGORITHM,
            Some("SHA1") => Algorithm::Sha1,
            Some("SHA256") => Algorithm::Sha256,
            Some("SHA512") => Algorithm::Sha512,
            Some(_) => return Err(OtpProblem::UnknownAlgorithm),
        };
        let digits = match digits.as_deref().map(|value| text(value)) {
            None => DEFAULT_DIGITS,
            Some(value) => match value.parse::<u32>() {
                Ok(n @ 6..=8) => n,
                _ => return Err(OtpProblem::BadDigits),
            },
        };
        let period = match period.as_deref().map(|value| text(value)) {
            None => DEFAULT_PERIOD,
            Some(value) => match value.parse::<u32>() {
                Ok(n @ 1..=MAX_PERIOD) => n,
                _ => return Err(OtpProblem::BadPeriod),
            },
        };
        Ok(Self { secret, algorithm, digits, period })
    }

    /// A bare base32 setup key, with the usual settings (SHA1, 6 digits,
    /// 30 seconds).
    pub fn from_setup_key(key: &str) -> Result<Self, OtpProblem> {
        let secret = base32(key.trim().as_bytes())?;
        Ok(Self { secret, algorithm: DEFAULT_ALGORITHM, digits: DEFAULT_DIGITS, period: DEFAULT_PERIOD })
    }

    pub fn digits(&self) -> u32 {
        self.digits
    }

    pub fn period(&self) -> u32 {
        self.period
    }

    /// The code at `unix_secs` seconds since 1970, zero-padded to
    /// [`Self::digits`].
    pub fn code_at(&self, unix_secs: u64) -> Zeroizing<String> {
        let counter = (unix_secs / u64::from(self.period)).to_be_bytes();
        let mac = match self.algorithm {
            Algorithm::Sha1 => mac::<Hmac<sha1_hmac::Sha1>>(&self.secret, &counter),
            Algorithm::Sha256 => mac::<Hmac<sha2::Sha256>>(&self.secret, &counter),
            Algorithm::Sha512 => mac::<Hmac<sha2::Sha512>>(&self.secret, &counter),
        };
        // Dynamic truncation, RFC 4226 section 5.3.
        let offset = usize::from(mac[mac.len() - 1] & 0x0f);
        let binary = u32::from_be_bytes([mac[offset] & 0x7f, mac[offset + 1], mac[offset + 2], mac[offset + 3]]);
        let code = binary % 10u32.pow(self.digits);
        Zeroizing::new(format!("{code:0width$}", width = self.digits as usize))
    }

    /// Seconds until the code at `unix_secs` changes: 1 to the period.
    pub fn seconds_left(&self, unix_secs: u64) -> u32 {
        self.period - (unix_secs % u64::from(self.period)) as u32
    }
}

fn mac<M: Mac + KeyInit>(key: &[u8], message: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut mac = <M as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message);
    Zeroizing::new(mac.finalize().into_bytes().to_vec())
}

fn label_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'@')
}

/// The `otpauth://` line to store for what was typed or pasted in the
/// editor: a pasted address once it is known to make codes, as pass-otp
/// reads it (see [`pass_otp_address`]), or for a bare base32 key the line
/// pass-otp writes,
/// `otpauth://totp/<label>?secret=<KEY>`, with the key in capitals and
/// without spaces or padding.
pub fn setup_line(input: &str, label: &str) -> Result<Zeroizing<String>, OtpProblem> {
    let input = input.trim();
    if is_address(input) {
        Totp::from_address(input)?;
        return pass_otp_address(input);
    }
    if input.contains(['\n', '\r', ':', '/', '?']) {
        return Err(OtpProblem::NotAnAddress);
    }
    Totp::from_setup_key(input)?;
    let mut line = Zeroizing::new(String::with_capacity(SCHEME.len() + 5 + label.len() * 3 + 8 + input.len()));
    line.push_str(SCHEME);
    line.push_str("totp/");
    for &byte in label.as_bytes() {
        if label_safe(byte) {
            line.push(char::from(byte));
        } else {
            line.push_str(&format!("%{byte:02X}"));
        }
    }
    line.push_str("?secret=");
    for byte in input.bytes().filter(|&b| !spacing(b) && b != b'=') {
        line.push(char::from(byte.to_ascii_uppercase()));
    }
    Ok(line)
}

/// A pasted `otpauth://totp` address in the form pass-otp reads, whose
/// pattern `^otpauth://(totp|hotp)/([^:?]+)?(:[^:?]*)?\?.+` is case-sensitive:
/// scheme and type in lowercase, and a `/` before the query when there is
/// no label. Everything after the type is kept as it was. A label with more
/// than one `:` is refused.
fn pass_otp_address(address: &str) -> Result<Zeroizing<String>, OtpProblem> {
    let (_, after) = split_type(&address[SCHEME.len()..]);
    let after = after.strip_prefix('/').unwrap_or(after);
    let Some((label, query)) = after.split_once('?') else { return Err(OtpProblem::NoSecret) };
    if query.is_empty() {
        return Err(OtpProblem::NoSecret);
    }
    if label.matches(':').count() > 1 {
        return Err(OtpProblem::BadLabel);
    }
    let mut line = Zeroizing::new(String::with_capacity(SCHEME.len() + 6 + after.len()));
    line.push_str(SCHEME);
    line.push_str("totp/");
    line.push_str(after);
    Ok(line)
}

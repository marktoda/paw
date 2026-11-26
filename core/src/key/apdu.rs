//! The card's protocol as paw speaks it (OpenPGP card 3.4 over ISO 7816-4):
//! the commands paw sends or watches for, and what the status words mean
//! (design.md 3.14). Everything here is pure.

use zeroize::Zeroizing;

use super::KeyError;

pub(crate) const SELECT: u8 = 0xA4;
pub(crate) const VERIFY: u8 = 0x20;
pub(crate) const GET_DATA: u8 = 0xCA;
/// PERFORM SECURITY OPERATION; with [`DECIPHER_P1`] and [`DECIPHER_P2`],
/// PSO:DECIPHER.
pub(crate) const PSO: u8 = 0x2A;
pub(crate) const DECIPHER_P1: u8 = 0x80;
pub(crate) const DECIPHER_P2: u8 = 0x86;
pub(crate) const INTERNAL_AUTHENTICATE: u8 = 0x88;
pub(crate) const GET_RESPONSE: u8 = 0xC0;

/// VERIFY's P1: check or verify, and forget.
pub(crate) const VERIFY_P1: u8 = 0x00;
pub(crate) const FORGET_P1: u8 = 0xFF;
/// VERIFY's P2 for PW1 in mode 82, which unlocks decryption and
/// authentication.
pub(crate) const PW1_82: u8 = 0x82;

/// The PW status bytes, whose fifth byte is the PW1 retry counter.
pub(crate) const PW_STATUS: u8 = 0xC4;
/// The PIN KDF data object.
pub(crate) const KDF: u8 = 0xF9;

/// VERIFY with no data: whether the PIN is verified. Uses no try.
pub(crate) const CHECK_PIN: [u8; 4] = [0x00, VERIFY, VERIFY_P1, PW1_82];
/// VERIFY with P1 `FF`: the card forgets that the PIN was verified.
pub(crate) const FORGET_PIN: [u8; 4] = [0x00, VERIFY, FORGET_P1, PW1_82];
pub(crate) const READ_PW_STATUS: [u8; 5] = [0x00, GET_DATA, 0x00, PW_STATUS, 0x00];
pub(crate) const READ_KDF: [u8; 5] = [0x00, GET_DATA, 0x00, KDF, 0x00];

/// VERIFY for PW1 in mode 82 carrying `data`, the PIN as the card wants it
/// (at most 255 bytes: a PIN's length is checked, and a KDF hash is at most
/// 64 bytes).
pub(crate) fn verify_pin(data: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut apdu = Zeroizing::new(Vec::with_capacity(5 + data.len()));
    apdu.extend_from_slice(&CHECK_PIN);
    apdu.push(data.len() as u8);
    apdu.extend_from_slice(data);
    apdu
}

pub(crate) fn carries_pin(header: [u8; 4], has_data: bool) -> bool {
    header[1] == VERIFY && header[2] == VERIFY_P1 && has_data
}

/// Whether a command carries a data field (ISO 7816-4 cases 3 and 4), as
/// opposed to none (case 1), or only an expected length: one byte (case 2)
/// or `00` and two bytes (extended case 2).
pub(crate) fn has_data(apdu: &[u8]) -> bool {
    match apdu.len() {
        0..=5 => false,
        7 => apdu[4] != 0x00,
        _ => true,
    }
}

/// PSO:DECIPHER and INTERNAL AUTHENTICATE are the commands a card holds
/// until it is touched; only the last (or only) part of a chain, CLA bit
/// 0x10 clear, is the one it holds.
pub(crate) fn waits_for_touch(apdu: &[u8]) -> bool {
    match apdu {
        [cla, PSO, DECIPHER_P1, DECIPHER_P2, ..] | [cla, INTERNAL_AUTHENTICATE, ..] => cla & 0x10 == 0,
        _ => false,
    }
}

pub(crate) const SW_OK: u16 = 0x9000;
pub(crate) const SW_TERMINATED: u16 = 0x6285;
pub(crate) const SW_WRONG_LENGTH: u16 = 0x6700;
pub(crate) const SW_SECURITY_NOT_SATISFIED: u16 = 0x6982;
pub(crate) const SW_PIN_BLOCKED: u16 = 0x6983;
pub(crate) const SW_CONDITIONS_NOT_SATISFIED: u16 = 0x6985;
pub(crate) const SW_WRONG_DATA: u16 = 0x6A80;
pub(crate) const SW_NOT_FOUND: u16 = 0x6A82;
pub(crate) const SW_WRONG_P1_P2: u16 = 0x6A86;
pub(crate) const SW_DATA_NOT_FOUND: u16 = 0x6A88;
pub(crate) const SW_INS_NOT_SUPPORTED: u16 = 0x6D00;
pub(crate) const SW_CLA_NOT_SUPPORTED: u16 = 0x6E00;
pub(crate) const SW_NO_DIAGNOSIS: u16 = 0x6F00;
/// `63Cx`: not verified, `x` tries left.
pub(crate) const SW_TRIES_LEFT: u16 = 0x63C0;

/// What the card said to a VERIFY, with or without a PIN in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verify {
    Verified,
    /// Not verified (or the PIN was wrong). `tries_left` is the card's
    /// counter if the status word carried it (`63Cx`); after `6982` it
    /// has to be read separately.
    NotVerified { tries_left: Option<u8> },
    Blocked,
    Other(u16),
}

pub(crate) fn verify(sw: u16) -> Verify {
    match sw {
        SW_OK => Verify::Verified,
        SW_PIN_BLOCKED => Verify::Blocked,
        sw if sw & 0xFFF0 == SW_TRIES_LEFT => match (sw & 0x000F) as u8 {
            0 => Verify::Blocked,
            tries => Verify::NotVerified { tries_left: Some(tries) },
        },
        SW_SECURITY_NOT_SATISFIED => Verify::NotVerified { tries_left: None },
        other => Verify::Other(other),
    }
}

/// Whether PSO:DECIPHER or INTERNAL AUTHENTICATE was refused for its
/// security conditions. A touch that never came and a PIN verification the
/// card lost both give that answer: the simulated card answers `6982` after
/// its touch timeout, and a YubiKey's answer has not been measured, so
/// `KeySession::refused` asks the card and looks at the time to tell them
/// apart.
pub(crate) fn security_refusal(sw: u16) -> bool {
    matches!(sw, SW_SECURITY_NOT_SATISFIED | SW_CONDITIONS_NOT_SATISFIED)
}

/// The error for a status word paw did not expect at `step`.
pub(crate) fn unexpected(step: &str, sw: u16) -> KeyError {
    let meaning = match sw {
        SW_WRONG_LENGTH | SW_WRONG_DATA => "it did not understand the command",
        SW_SECURITY_NOT_SATISFIED => "the PIN is not verified",
        SW_NOT_FOUND | SW_WRONG_P1_P2 => "it has no OpenPGP application",
        SW_DATA_NOT_FOUND => "the data is not there",
        SW_INS_NOT_SUPPORTED | SW_CLA_NOT_SUPPORTED => "it does not support the command",
        SW_TERMINATED => "its OpenPGP application is terminated",
        _ => "it refused",
    };
    KeyError::Reader { detail: format!("{step}: {meaning} ({sw:04X})") }
}

/// The instruction, named, with P1 and P2 where they say something.
pub(crate) fn command_words(header: [u8; 4], has_data: bool) -> String {
    let [cla, ins, p1, p2] = header;
    let name = match (ins, p1, p2) {
        (SELECT, ..) => "SELECT".to_string(),
        (GET_DATA | 0xCB, ..) => format!("GET DATA {p1:02X}{p2:02X}"),
        (VERIFY, FORGET_P1, _) => format!("VERIFY {p1:02X} {p2:02X} (forget)"),
        (VERIFY, ..) => format!("VERIFY {p1:02X} {p2:02X} ({})", if has_data { "with PIN" } else { "check" }),
        (PSO, DECIPHER_P1, DECIPHER_P2) => "PSO:DECIPHER".into(),
        (PSO, 0x9E, 0x9A) => "PSO:COMPUTE SIGNATURE".into(),
        (PSO, ..) => format!("PSO {p1:02X} {p2:02X}"),
        (INTERNAL_AUTHENTICATE, ..) => "INTERNAL AUTHENTICATE".into(),
        (GET_RESPONSE, ..) => "GET RESPONSE".into(),
        (0x47, 0x81, _) => "READ PUBLIC KEY".into(),
        (0x47, ..) => "GENERATE KEY".into(),
        (0xDA | 0xDB, ..) => format!("PUT DATA {p1:02X}{p2:02X}"),
        (0x24, ..) => format!("CHANGE PIN {p2:02X}"),
        (0x2C, ..) => format!("RESET RETRY COUNTER {p2:02X}"),
        (0x84, ..) => "GET CHALLENGE".into(),
        (0xE6, ..) => "TERMINATE".into(),
        (0x44, ..) => "ACTIVATE".into(),
        _ => format!("INS {ins:02X} {p1:02X} {p2:02X}"),
    };
    if cla & 0x10 != 0 { format!("{name} (chain part)") } else { name }
}

/// A status word as hex, with the length byte of `61xx` and `6Cxx` (how
/// much response data there is) left out.
pub(crate) fn sw_code(sw: u16) -> String {
    match sw & 0xFF00 {
        0x6100 => "61xx".into(),
        0x6C00 => "6Cxx".into(),
        _ => format!("{sw:04X}"),
    }
}

pub(crate) fn sw_words(sw: u16) -> String {
    match sw {
        SW_OK => "ok".into(),
        // Their low byte is a length of response data: not shown.
        sw if sw & 0xFF00 == 0x6100 => "more data".into(),
        sw if sw & 0xFF00 == 0x6C00 => "wrong length".into(),
        SW_TRIES_LEFT => "blocked, 0 tries left".into(),
        sw if sw & 0xFFF0 == SW_TRIES_LEFT => format!("not verified, {} tries left", sw & 0xF),
        SW_TERMINATED => "application terminated".into(),
        0x6400 => "execution error".into(),
        0x6581 => "memory failure".into(),
        SW_WRONG_LENGTH => "wrong length".into(),
        SW_SECURITY_NOT_SATISFIED => "security status not satisfied".into(),
        SW_PIN_BLOCKED => "PIN blocked".into(),
        0x6984 => "data invalid".into(),
        SW_CONDITIONS_NOT_SATISFIED => "conditions not satisfied".into(),
        SW_WRONG_DATA => "wrong data".into(),
        SW_NOT_FOUND => "not found".into(),
        SW_WRONG_P1_P2 | 0x6B00 => "wrong P1 or P2".into(),
        SW_DATA_NOT_FOUND => "data not found".into(),
        SW_INS_NOT_SUPPORTED => "instruction not supported".into(),
        SW_CLA_NOT_SUPPORTED => "class not supported".into(),
        SW_NO_DIAGNOSIS => "no precise diagnosis".into(),
        _ => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tries_from_63cx() {
        assert_eq!(verify(0x63C2), Verify::NotVerified { tries_left: Some(2) });
        assert_eq!(verify(0x63C0), Verify::Blocked);
        assert_eq!(verify(0x6983), Verify::Blocked);
        assert_eq!(verify(0x6982), Verify::NotVerified { tries_left: None });
        assert_eq!(verify(0x9000), Verify::Verified);
        assert_eq!(verify(0x6A80), Verify::Other(0x6A80));
    }

    #[test]
    fn only_a_data_field_counts_as_data() {
        assert!(!has_data(&[0x00, 0x20, 0x00, 0x82]));
        assert!(!has_data(&[0x00, 0xCA, 0x00, 0x6E, 0x00]));
        assert!(!has_data(&[0x00, 0xCA, 0x00, 0x6E, 0x00, 0x00, 0x00]));
        assert!(has_data(&[0x00, 0x20, 0x00, 0x82, 0x01, 0x31]));
        assert!(has_data(&[0x00, 0x20, 0x00, 0x82, 0x02, 0x31, 0x32]));
        assert!(has_data(&[0x00, 0x2A, 0x80, 0x86, 0x00, 0x00, 0x01, 0xA6, 0x00, 0x00]));
    }

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

    #[test]
    fn the_verify_paw_sends_is_the_one_the_log_sees_as_carrying_the_pin() {
        let apdu = verify_pin(b"123456");
        let header = [apdu[0], apdu[1], apdu[2], apdu[3]];
        assert!(carries_pin(header, has_data(&apdu)));
        assert!(!carries_pin(CHECK_PIN, has_data(&CHECK_PIN)));
        assert!(!carries_pin(FORGET_PIN, has_data(&FORGET_PIN)));
        assert_eq!(command_words(header, true), "VERIFY 00 82 (with PIN)");
    }

    #[test]
    fn names_commands_without_their_data() {
        assert_eq!(command_words([0x00, 0x20, 0x00, 0x82], false), "VERIFY 00 82 (check)");
        assert_eq!(command_words([0x00, 0x20, 0xFF, 0x82], false), "VERIFY FF 82 (forget)");
        assert_eq!(command_words([0x10, 0x2A, 0x80, 0x86], true), "PSO:DECIPHER (chain part)");
        assert_eq!(command_words([0x00, 0xCA, 0x00, 0x6E], false), "GET DATA 006E");
        assert_eq!(command_words([0x00, 0x99, 0x01, 0x02], false), "INS 99 01 02");
    }

    #[test]
    fn status_words_that_carry_a_count_or_a_length_say_so() {
        assert_eq!(sw_words(0x63C2), "not verified, 2 tries left");
        assert_eq!(sw_words(0x63C0), "blocked, 0 tries left");
        assert_eq!(sw_code(0x6110), "61xx");
        assert_eq!(sw_code(0x6C20), "6Cxx");
        assert_eq!(sw_code(0x6982), "6982");
    }
}

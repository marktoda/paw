//! What the card's status words mean, in one place (design.md 3.14).
//!
//! Everything here is pure: a status word and what paw knows about the
//! command in, a meaning out.

use super::KeyError;

pub(crate) const OK: u16 = 0x9000;

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
        OK => Verify::Verified,
        0x6983 => Verify::Blocked,
        sw if sw & 0xFFF0 == 0x63C0 => match (sw & 0x000F) as u8 {
            0 => Verify::Blocked,
            tries => Verify::NotVerified { tries_left: Some(tries) },
        },
        0x6982 => Verify::NotVerified { tries_left: None },
        other => Verify::Other(other),
    }
}

/// What the card said to a command that may wait for a touch (PSO:DECIPHER
/// or INTERNAL AUTHENTICATE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyOperation {
    Done,
    TouchTimeout,
    Refused(u16),
}

/// **Unverified on a real key.** The YubiKey's status word for "nobody
/// touched me" has not been measured, and the simulated card answers
/// `6982` after its timeout. So "a touch may have been waited for, and the
/// card then refused with a security-status error" is only a candidate
/// touch timeout: [`super::session::KeySession::refused`] then asks the
/// card whether the PIN is still verified and looks at how long the
/// refusal took before calling it one.
pub(crate) fn touch_timed_out(touch_possible: bool, sw: u16) -> bool {
    touch_possible && matches!(sw, 0x6982 | 0x6985)
}

pub(crate) fn key_operation(sw: u16, touch_possible: bool) -> KeyOperation {
    if sw == OK {
        KeyOperation::Done
    } else if touch_timed_out(touch_possible, sw) {
        KeyOperation::TouchTimeout
    } else {
        KeyOperation::Refused(sw)
    }
}

/// The error for a status word paw did not expect at `step`.
pub(crate) fn unexpected(step: &str, sw: u16) -> KeyError {
    let meaning = match sw {
        0x6700 | 0x6A80 => "it did not understand the command",
        0x6982 => "the PIN is not verified",
        0x6A82 | 0x6A86 => "it has no OpenPGP application",
        0x6A88 => "the data is not there",
        0x6D00 | 0x6E00 => "it does not support the command",
        0x6285 => "its OpenPGP application is terminated",
        _ => "it refused",
    };
    KeyError::Reader { detail: format!("{step}: {meaning} ({sw:04X})") }
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
    fn a_security_refusal_is_a_touch_timeout_only_if_a_touch_was_possible() {
        assert_eq!(key_operation(0x6982, true), KeyOperation::TouchTimeout);
        assert_eq!(key_operation(0x6985, true), KeyOperation::TouchTimeout);
        assert_eq!(key_operation(0x6982, false), KeyOperation::Refused(0x6982));
        assert_eq!(key_operation(0x6A80, true), KeyOperation::Refused(0x6A80));
        assert_eq!(key_operation(0x9000, true), KeyOperation::Done);
    }
}

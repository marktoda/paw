//! One-time codes: RFC 6238's own test vectors, the addresses pass-otp and
//! authenticator apps write, and that nothing about the secret leaks.

use paw_core::entry::EntryDoc;
use paw_core::entry::opened::OpenedEntry;
use paw_core::otp::{OtpProblem, Totp, setup_line};
use proptest::prelude::*;

/// The seeds of RFC 6238 appendix B, in base32.
const SEED_SHA1: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
const SEED_SHA256: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZA";
const SEED_SHA512: &str =
    "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNA";

fn totp(address: &str) -> Totp {
    Totp::from_address(address).unwrap()
}

#[test]
fn rfc_6238_appendix_b_vectors() {
    let table: [(u64, &str, &str, &str); 6] = [
        (59, "94287082", "46119246", "90693936"),
        (1_111_111_109, "07081804", "68084774", "25091201"),
        (1_111_111_111, "14050471", "67062674", "99943326"),
        (1_234_567_890, "89005924", "91819424", "93441116"),
        (2_000_000_000, "69279037", "90698825", "38618901"),
        (20_000_000_000, "65353130", "77737706", "47863826"),
    ];
    let sha1 = totp(&format!("otpauth://totp/rfc?secret={SEED_SHA1}&digits=8"));
    let sha256 = totp(&format!("otpauth://totp/rfc?secret={SEED_SHA256}&digits=8&algorithm=SHA256"));
    let sha512 = totp(&format!("otpauth://totp/rfc?secret={SEED_SHA512}&digits=8&algorithm=SHA512"));
    for (time, one, two, five) in table {
        assert_eq!(sha1.code_at(time).as_str(), one, "SHA1 at {time}");
        assert_eq!(sha256.code_at(time).as_str(), two, "SHA256 at {time}");
        assert_eq!(sha512.code_at(time).as_str(), five, "SHA512 at {time}");
    }
}

#[test]
fn the_common_example_key_with_defaults() {
    // Worked out independently with Python's hmac module.
    let code = totp("otpauth://totp/GitHub:you?secret=JBSWY3DPEHPK3PXP&issuer=GitHub");
    assert_eq!((code.digits(), code.period()), (6, 30));
    for (time, expected) in [(0, "282760"), (59, "996554"), (1_111_111_109, "071271"), (1_700_000_000, "324550")] {
        assert_eq!(code.code_at(time).as_str(), expected, "at {time}");
    }
    assert_eq!(code.seconds_left(1_700_000_000), 10);
    assert_eq!(code.seconds_left(1_700_000_010), 30);
    assert_eq!(code.seconds_left(1_700_000_009), 1);
}

#[test]
fn secrets_are_read_the_way_people_copy_them() {
    let reference = totp("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP").code_at(1_700_000_000);
    for secret in ["jbswy3dpehpk3pxp", "JBSW Y3DP EHPK 3PXP", "JBSWY3DPEHPK3PXP%3D%3D", "JBSW-Y3DP-EHPK-3PXP"] {
        let address = format!("otpauth://totp/x?secret={secret}");
        assert_eq!(totp(&address).code_at(1_700_000_000), reference, "{secret}");
    }
    // Padding, letter case in the scheme and the parameter names.
    let padded = totp("OTPAUTH://TOTP/x?SECRET=MZXW6===&Period=60&DIGITS=7");
    assert_eq!((padded.digits(), padded.period()), (7, 60));
    assert_eq!(Totp::from_setup_key(" jbsw y3dp ehpk 3pxp ").unwrap().code_at(1_700_000_000), reference);
}

#[test]
fn unusable_lines_say_why() {
    let problem = |address: &str| Totp::from_address(address).unwrap_err();
    assert_eq!(problem("otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP&counter=1"), OtpProblem::CounterBased);
    assert_eq!(problem("otpauth://totp/x?issuer=GitHub"), OtpProblem::NoSecret);
    assert_eq!(problem("otpauth://totp/x"), OtpProblem::NoSecret);
    assert_eq!(problem("otpauth://totp/x?secret="), OtpProblem::NoSecret);
    assert_eq!(problem("otpauth://totp/x?secret=JBSWY3DP1"), OtpProblem::BadSecret);
    assert_eq!(problem("otpauth://totp/x?secret=A"), OtpProblem::BadSecret);
    assert_eq!(problem("otpauth://totp/x?secret=AB=CD"), OtpProblem::BadSecret);
    assert_eq!(problem("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&algorithm=MD5"), OtpProblem::UnknownAlgorithm);
    assert_eq!(problem("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=5"), OtpProblem::BadDigits);
    assert_eq!(problem("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=9"), OtpProblem::BadDigits);
    assert_eq!(problem("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=six"), OtpProblem::BadDigits);
    assert_eq!(problem("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=0"), OtpProblem::BadPeriod);
    assert_eq!(problem("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=86400"), OtpProblem::BadPeriod);
    assert_eq!(problem("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=-30"), OtpProblem::BadPeriod);
    assert_eq!(problem("otpauth://steam/x?secret=JBSWY3DPEHPK3PXP"), OtpProblem::NotAnAddress);
    assert_eq!(problem("https://example.com"), OtpProblem::NotAnAddress);
    for reason in [OtpProblem::CounterBased, OtpProblem::BadSecret, OtpProblem::BadPeriod] {
        assert!(reason.reason().ends_with('.'));
    }
}

#[test]
fn the_secret_is_never_in_debug_output() {
    let code = totp("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP");
    let debug = format!("{code:?}");
    assert!(!debug.contains("JBSW") && !debug.contains("72, 101"), "{debug}");
    let opened = OpenedEntry::decrypted("x", b"pw\notpauth://totp/x?secret=JBSWY3DPEHPK3PXP\n".to_vec(), vec![0; 32]).unwrap();
    let shown = opened.otp_code(1_700_000_000_000).unwrap();
    assert!(!format!("{shown:?}").contains("324550"));
}

#[test]
fn an_opened_entry_makes_its_code_from_the_clock_it_is_given() {
    let opened = OpenedEntry::decrypted("x", 
        b"pw\nlogin: you\notpauth://totp/GitHub:you?secret=JBSWY3DPEHPK3PXP&issuer=GitHub\n".to_vec(), vec![0; 32])
    .unwrap();
    assert!(opened.view().has_otp);
    let code = opened.otp_code(1_700_000_000_500).unwrap();
    assert_eq!((code.code.as_str(), code.seconds_left, code.period), ("324550", 10, 30));
    assert_eq!(opened.otp_problem(), None);
    // Before 1970 counts as 1970.
    assert_eq!(opened.otp_code(-5).unwrap().code, "282760");
    opened.close();
    assert_eq!(opened.otp_code(1_700_000_000_000), None);

    // The `key: address` form some tools write.
    let keyed = OpenedEntry::decrypted("x", b"pw\notpauth: otpauth://totp/x?secret=JBSWY3DPEHPK3PXP\n".to_vec(), vec![0; 32]).unwrap();
    assert_eq!(keyed.otp_code(1_700_000_000_000).unwrap().code, "324550");

    let counter = OpenedEntry::decrypted("x", b"pw\notpauth://hotp/x?secret=JBSWY3DPEHPK3PXP&counter=3\n".to_vec(), vec![0; 32]).unwrap();
    assert!(counter.view().has_otp);
    assert_eq!(counter.otp_code(1_700_000_000_000), None);
    assert_eq!(counter.otp_problem().as_deref(), Some("Counter-based codes aren't supported."));

    let none = OpenedEntry::decrypted("x", b"pw\nlogin: you\n".to_vec(), vec![0; 32]).unwrap();
    assert_eq!((none.otp_code(0), none.otp_problem()), (None, None));
}

proptest! {
    /// AutoFill fills the next code when the current one has under 3
    /// seconds left: `otp_code(now + seconds_left * 1000 + 1)` is always
    /// the first code of the next period.
    #[test]
    fn a_step_of_seconds_left_lands_in_the_next_period(now_ms in 0i64..4_000_000_000_000) {
        let opened = OpenedEntry::decrypted("x", b"pw\notpauth://totp/x?secret=JBSWY3DPEHPK3PXP\n".to_vec(), vec![0; 32]).unwrap();
        let now = opened.otp_code(now_ms).unwrap();
        let next = opened.otp_code(now_ms + i64::from(now.seconds_left) * 1000 + 1).unwrap();
        let next_period = (now_ms / 1000 / 30 + 1) * 30;
        let expected = totp("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP").code_at(next_period as u64);
        prop_assert_eq!(next.code.as_str(), expected.as_str());
        prop_assert!(next.seconds_left >= 29, "{}", next.seconds_left);
    }
}

#[test]
fn the_first_one_time_code_line_is_the_one_used() {
    let doc = EntryDoc::parse(
        b"pw\notpauth://totp/a?secret=JBSWY3DPEHPK3PXP\notpauth://totp/b?secret=MZXW6YQ\n".to_vec(),
    )
    .unwrap();
    assert_eq!(doc.one_time_code().unwrap().unwrap().code_at(1_700_000_000).as_str(), "324550");
}

#[test]
fn a_bare_key_becomes_the_line_pass_otp_writes() {
    assert_eq!(
        setup_line("jbsw y3dp ehpk 3pxp", "github.com").unwrap().as_str(),
        "otpauth://totp/github.com?secret=JBSWY3DPEHPK3PXP"
    );
    assert_eq!(
        setup_line("JBSWY3DPEHPK3PXP===", "Home Wi-Fi: 5G?").unwrap().as_str(),
        "otpauth://totp/Home%20Wi-Fi%3A%205G%3F?secret=JBSWY3DPEHPK3PXP"
    );
    // A pasted address is kept as it is, once it is known to work.
    let pasted = "otpauth://totp/GitHub:you?secret=JBSWY3DPEHPK3PXP&issuer=GitHub&period=60";
    assert_eq!(setup_line(&format!("  {pasted}\n"), "ignored").unwrap().as_str(), pasted);
    assert_eq!(setup_line("otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP", "x").unwrap_err(), OtpProblem::CounterBased);
    assert_eq!(setup_line("not a key!", "x").unwrap_err(), OtpProblem::BadSecret);
    assert_eq!(setup_line("https://example.com/2fa", "x").unwrap_err(), OtpProblem::NotAnAddress);
}

#[test]
fn a_pasted_address_is_written_so_pass_otp_can_read_it() {
    let line = |pasted: &str| setup_line(pasted, "ignored").map(|l| l.as_str().to_owned());
    assert_eq!(
        line("OTPAUTH://TOTP/GitHub:you?secret=JBSWY3DPEHPK3PXP").unwrap(),
        "otpauth://totp/GitHub:you?secret=JBSWY3DPEHPK3PXP"
    );
    assert_eq!(line("otpauth://totp?secret=JBSWY3DPEHPK3PXP").unwrap(), "otpauth://totp/?secret=JBSWY3DPEHPK3PXP");
    assert_eq!(line("otpauth://Totp/?secret=JBSWY3DPEHPK3PXP").unwrap(), "otpauth://totp/?secret=JBSWY3DPEHPK3PXP");
    assert_eq!(line("otpauth://totp/:you?secret=JBSWY3DPEHPK3PXP").unwrap(), "otpauth://totp/:you?secret=JBSWY3DPEHPK3PXP");
    assert_eq!(line("otpauth://totp/a:b:c?secret=JBSWY3DPEHPK3PXP").unwrap_err(), OtpProblem::BadLabel);
}

proptest! {
    /// Anything at all gives a code or a reason, never a panic, and a code
    /// always has the digits asked for.
    #[test]
    fn any_address_gives_a_code_or_a_reason(
        query in "[a-zA-Z0-9=&%: ]{0,60}",
        kind in "(totp|hotp|TOTP|x)",
        time in any::<u64>(),
    ) {
        if let Ok(code) = Totp::from_address(&format!("otpauth://{kind}/x?{query}")) {
            let made = code.code_at(time);
            prop_assert_eq!(made.len() as u32, code.digits());
            prop_assert!(made.bytes().all(|b| b.is_ascii_digit()));
            prop_assert!((1..=code.period()).contains(&code.seconds_left(time)));
        }
    }

    /// A typed key and the line made from it give the same codes.
    #[test]
    fn a_saved_key_makes_the_codes_it_was_typed_with(key in "[A-Z2-7]{16,32}", time in 0u64..4_000_000_000) {
        prop_assume!(!matches!(key.len() % 8, 1 | 3 | 6));
        let line = setup_line(&key.to_lowercase(), "x").unwrap();
        prop_assert_eq!(
            Totp::from_address(&line).unwrap().code_at(time),
            Totp::from_setup_key(&key).unwrap().code_at(time)
        );
    }
}

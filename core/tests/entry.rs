//! The text of an opened entry: read by `pass` conventions, written back
//! byte for byte, and never printed.

use paw_core::PawError;
use paw_core::entry::EntryDoc;
use proptest::prelude::*;

const TYPICAL: &str = "tidal-Quartz-88\nlogin: you@example.com\nURL: https://github.com/login\n\
security answer: Biscuit\notpauth://totp/GitHub:you?secret=JBSWY3DPEHPK3PXP\n\
\nRecovery codes are in the blue notebook.\nhttps://github.com/settings/security\n";

fn parse(text: &str) -> EntryDoc {
    EntryDoc::parse(text.as_bytes().to_vec()).unwrap()
}

fn text(doc: &EntryDoc) -> String {
    String::from_utf8(doc.to_bytes().to_vec()).unwrap()
}

#[test]
fn the_first_line_is_the_password() {
    assert_eq!(parse(TYPICAL).password(), "tidal-Quartz-88");
    assert_eq!(parse("only a password").password(), "only a password");
    // Spaces are part of a password.
    assert_eq!(parse("  two words \nlogin: you\n").password(), "  two words ");
}

#[test]
fn the_password_may_be_empty() {
    assert_eq!(parse("").password(), "");
    let fields_only = parse("\nlogin: you@example.com\n");
    assert_eq!(fields_only.password(), "");
    assert_eq!(fields_only.username(), Some("you@example.com"));
}

#[test]
fn the_username_is_the_first_known_key_in_any_letter_case() {
    let cases = [
        ("pw\nlogin: a\nusername: b\n", Some("a")),
        ("pw\nusername: b\nlogin: a\n", Some("a")),
        ("pw\nUserName: b\nemail: c\n", Some("b")),
        ("pw\nUSER:  d  \n", Some("d")),
        ("pw\nE-mail: no\nemail: c\n", Some("c")),
        ("pw\nlogin:\n", Some("")),
        ("pw\nnickname: e\n", None),
        // The password line is never a field, whatever it looks like.
        ("login: not-a-username\n", None),
    ];
    for (entry, expected) in cases {
        assert_eq!(parse(entry).username(), expected, "{entry:?}");
    }
}

#[test]
fn the_url_is_the_first_known_key_in_any_letter_case() {
    let cases = [
        ("pw\nurl: https://example.com/a?b=c\n", Some("https://example.com/a?b=c")),
        ("pw\nwebsite: w\nURL: u\n", Some("u")),
        ("pw\nsite: s\nweb: w\nlink: l\n", Some("l")),
        ("pw\nWebsite: example.com\n", Some("example.com")),
        ("pw\nhttps://example.com\n", None),
    ];
    for (entry, expected) in cases {
        assert_eq!(parse(entry).url(), expected, "{entry:?}");
    }
}

#[test]
fn other_fields_keep_their_order_and_their_spelling() {
    let doc = parse(TYPICAL);
    let fields: Vec<_> = doc.other_fields().iter().map(|f| (f.key, f.value)).collect();
    assert_eq!(fields, [("security answer", "Biscuit")]);

    let several = parse("pw\nPort: 2222\nlogin: a\nuser: b\nsite: s\nurl: u\nPIN: 1\n");
    let fields: Vec<_> = several.other_fields().iter().map(|f| (f.key, f.value)).collect();
    // `login` and `url` are shown in their own rows; the rest stay here.
    assert_eq!(fields, [("Port", "2222"), ("user", "b"), ("site", "s"), ("PIN", "1")]);
    assert_eq!(several.field("port"), Some("2222"));
    assert_eq!(several.field("missing"), None);
}

#[test]
fn fields_that_sound_secret_start_hidden() {
    let doc = parse("pw\nsecurity answer: x\ncard PIN: 1\nPassphrase: y\nport: 22\nclient secret: z\n");
    let hidden: Vec<_> = doc.other_fields().iter().map(|f| (f.key, f.looks_secret())).collect();
    assert_eq!(
        hidden,
        [
            ("security answer", true),
            ("card PIN", true),
            ("Passphrase", true),
            ("port", false),
            ("client secret", true)
        ]
    );
}

#[test]
fn a_one_time_code_address_is_noticed_but_is_not_a_field_or_a_note() {
    for entry in [
        TYPICAL,
        "pw\nOTPAUTH://totp/x?secret=ABC\n",
        "pw\notpauth: otpauth://totp/x?secret=ABC\n",
        "pw\n  otpauth://hotp/x?secret=ABC&counter=1",
    ] {
        let doc = parse(entry);
        assert!(doc.has_otp(), "{entry:?}");
        assert!(doc.other_fields().iter().all(|f| !f.value.contains("secret=")));
        assert!(!doc.notes().contains("secret="));
    }
    assert!(!parse("otpauth://totp/x?secret=ABC\n").has_otp(), "the first line is the password");
    assert!(!parse("pw\nlogin: you\n").has_otp());
}

#[test]
fn notes_are_the_lines_that_are_nothing_else() {
    assert_eq!(
        parse(TYPICAL).notes().as_str(),
        "Recovery codes are in the blue notebook.\nhttps://github.com/settings/security"
    );
    // Blank lines inside a note stay; those around it go.
    assert_eq!(parse("pw\n\n\nfirst\n\nsecond\n\n").notes().as_str(), "first\n\nsecond");
    assert_eq!(parse("pw\nlogin: you\n").notes().as_str(), "");
    assert_eq!(parse("pw\n: no key\n").notes().as_str(), ": no key");
}

#[test]
fn writes_back_exactly_what_it_read() {
    for entry in [
        "",
        "\n",
        "pw",
        "pw\n",
        "pw\r\nlogin: you\r\n",
        "pw\nlogin: you\r\nurl:x\n\n\nlast line without an end",
        "pw\rstill the password\n",
        "pw\nkey :  spaced \t\n",
        TYPICAL,
    ] {
        assert_eq!(text(&parse(entry)), entry, "{entry:?}");
    }
}

#[test]
fn bytes_that_are_not_text_are_an_error_not_a_panic() {
    for bytes in [vec![0xFF, 0xFE, 0x00], b"pw\nlogin: \xC3\x28\n".to_vec(), vec![0x80; 4096]] {
        assert!(matches!(EntryDoc::parse(bytes), Err(PawError::NotText)));
    }
}

#[test]
fn changing_the_password_changes_only_the_first_line() {
    let mut doc = parse(TYPICAL);
    doc.set_password("a new one").unwrap();
    assert_eq!(text(&doc), TYPICAL.replacen("tidal-Quartz-88", "a new one", 1));

    let mut crlf = parse("old\r\nlogin: you\r\n");
    crlf.set_password("new").unwrap();
    assert_eq!(text(&crlf), "new\r\nlogin: you\r\n");

    let mut empty = parse("");
    empty.set_password("first").unwrap();
    assert_eq!(text(&empty), "first\n");
}

#[test]
fn changing_a_field_changes_only_that_line() {
    let cases = [
        // (before, key, value, after)
        ("pw\nlogin: old\nurl: u\n", "login", "new", "pw\nlogin: new\nurl: u\n"),
        ("pw\nLogin:\told\r\nurl: u\r\n", "LOGIN", "new", "pw\nLogin:\tnew\r\nurl: u\r\n"),
        ("pw\nuser:old\nuser: second\n", "user", "new", "pw\nuser:new\nuser: second\n"),
        ("pw\nport :  22  \nnote\n", "port", "2222", "pw\nport :  2222\nnote\n"),
        ("pw\nurl: https://a.example/x:y\n", "url", "https://b.example", "pw\nurl: https://b.example\n"),
        ("pw\nlogin: old", "login", "new", "pw\nlogin: new"),
    ];
    for (before, key, value, after) in cases {
        let mut doc = parse(before);
        doc.set_field(key, value).unwrap();
        assert_eq!(text(&doc), after, "{before:?}");
    }
}

#[test]
fn a_new_field_goes_on_a_new_last_line() {
    let cases = [
        ("pw\nlogin: you\n", "pw\nlogin: you\nurl: example.com\n"),
        ("pw\r\nlogin: you\r\n", "pw\r\nlogin: you\r\nurl: example.com\r\n"),
        ("pw", "pw\nurl: example.com\n"),
        ("", "\nurl: example.com\n"),
    ];
    for (before, after) in cases {
        let mut doc = parse(before);
        doc.set_field("url", "example.com").unwrap();
        assert_eq!(text(&doc), after, "{before:?}");
        assert_eq!(doc.url(), Some("example.com"));
    }
}

#[test]
fn refuses_values_that_would_become_other_lines() {
    let mut doc = parse("pw\nlogin: you\n");
    for (key, value) in [
        ("login", "a\nextra: line"),
        ("login", "a\r"),
        ("lo\ngin", "a"),
        ("with:colon", "a"),
        ("  ", "a"),
        ("link", "//example.com"),
        ("code", "otpauth://totp/x?secret=ABC"),
    ] {
        assert!(matches!(doc.set_field(key, value), Err(PawError::InvalidField(_))), "{key:?}");
    }
    assert!(matches!(doc.set_password("a\nb"), Err(PawError::InvalidField(_))));
    assert_eq!(text(&doc), "pw\nlogin: you\n");
}

#[test]
fn debug_output_shows_no_contents() {
    let doc = parse(TYPICAL);
    let printed = format!("{doc:?} {doc:#?}");
    for secret in ["tidal-Quartz-88", "you@example.com", "Biscuit", "JBSWY3DPEHPK3PXP", "notebook"] {
        assert!(!printed.contains(secret), "{printed}");
    }
    assert!(printed.contains("8 lines"));
}

/// Text made of the pieces that matter to the parser, so that most cases
/// exercise fields, endings and odd spacing rather than random letters.
fn entry_like() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("\n".to_owned()),
        Just("\r\n".to_owned()),
        Just("\r".to_owned()),
        Just(": ".to_owned()),
        Just(":".to_owned()),
        Just("login".to_owned()),
        Just("otpauth://totp/x?secret=A".to_owned()),
        Just("https://example.com".to_owned()),
        "[ \ta-zA-Z0-9é€#/]{0,6}",
    ];
    proptest::collection::vec(piece, 0..40).prop_map(|pieces| pieces.concat())
}

proptest! {
    #[test]
    fn any_bytes_parse_or_fail_cleanly_and_text_round_trips(
        bytes in proptest::collection::vec(any::<u8>(), 0..300)
    ) {
        match EntryDoc::parse(bytes.clone()) {
            Ok(doc) => prop_assert_eq!(doc.to_bytes().to_vec(), bytes),
            Err(e) => {
                prop_assert!(matches!(e, PawError::NotText));
                prop_assert!(std::str::from_utf8(&bytes).is_err());
            }
        }
    }

    #[test]
    fn any_text_round_trips(entry in any::<String>()) {
        prop_assert_eq!(text(&parse(&entry)), entry);
    }

    #[test]
    fn entry_shaped_text_round_trips_and_every_accessor_is_safe(entry in entry_like()) {
        let doc = parse(&entry);
        prop_assert_eq!(text(&doc), entry.clone());
        // None of these may panic, whatever the text.
        let _ = (doc.password(), doc.username(), doc.url(), doc.has_otp(), doc.notes());
        for field in doc.other_fields() {
            prop_assert!(!field.key.is_empty());
        }
    }

    #[test]
    fn a_password_change_leaves_every_other_line_alone(
        entry in entry_like(),
        password in "[ -~]{0,20}",
    ) {
        let mut doc = parse(&entry);
        doc.set_password(&password).unwrap();
        let after = text(&doc);
        prop_assert_eq!(doc.password(), password.as_str());
        match entry.split_once('\n') {
            Some((first, rest)) => {
                let ending = if first.ends_with('\r') { "\r\n" } else { "\n" };
                prop_assert_eq!(after, format!("{password}{ending}{rest}"));
            }
            None if entry.is_empty() => prop_assert_eq!(after, format!("{password}\n")),
            None => prop_assert_eq!(after, password),
        }
    }

    #[test]
    fn a_field_change_leaves_every_other_line_alone(
        entry in entry_like(),
        value in "[a-z0-9 @.]{0,12}",
    ) {
        let mut doc = parse(&entry);
        let had = doc.field("login").is_some();
        doc.set_field("login", &value).unwrap();
        prop_assert_eq!(doc.field("login"), Some(value.trim()));

        let after = text(&doc);
        let before: Vec<&str> = entry.split_inclusive('\n').collect();
        let now: Vec<&str> = after.split_inclusive('\n').collect();
        if had {
            prop_assert_eq!(before.len(), now.len());
            let changed = before.iter().zip(&now).filter(|(a, b)| a != b).count();
            prop_assert!(changed <= 1);
        } else {
            // One new last line; at most a line ending added before it.
            let kept = before.len().max(1);
            prop_assert_eq!(now.len(), kept + 1);
            for (a, b) in before.iter().zip(&now).take(before.len().saturating_sub(1)) {
                prop_assert_eq!(a, b);
            }
        }
    }
}

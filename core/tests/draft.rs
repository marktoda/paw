//! The add and edit form before it is saved: what it refuses, in words a
//! person can act on, and that an edit rewrites only the lines it touches.

use paw_core::entry::EntryDoc;
use paw_core::store::tree::StoreTree;
use paw_core::write::{DraftField, DraftProblem, EntryDraft};
use proptest::prelude::*;

fn parse(text: &str) -> EntryDoc {
    EntryDoc::parse(text.as_bytes().to_vec()).unwrap()
}

fn text(doc: &EntryDoc) -> String {
    String::from_utf8(doc.to_bytes().to_vec()).unwrap()
}

fn edit(text: &str) -> EntryDraft {
    EntryDraft::edit("Work/github.com", parse(text), vec![0; 32])
}

fn saved(draft: &EntryDraft) -> String {
    text(&draft.build().unwrap())
}

/// A store with `Work/github.com`, `Work/Jira`, `Home Wi-Fi` and a folder
/// `Work/servers`.
fn tree() -> (tempfile::TempDir, StoreTree) {
    let dir = tempfile::tempdir().unwrap();
    for path in ["Work/github.com.gpg", "Work/Jira.gpg", "Home Wi-Fi.gpg", "Work/servers/box.gpg"] {
        let file = dir.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, b"x").unwrap();
    }
    let tree = StoreTree::scan(dir.path()).unwrap();
    (dir, tree)
}

fn messages(problems: &[DraftProblem]) -> Vec<&str> {
    problems.iter().map(|p| p.message.as_str()).collect()
}

fn name_problem(name: &str) -> Vec<String> {
    let mut draft = EntryDraft::new("Work");
    draft.set_name(name);
    draft.problems(None).into_iter().map(|p| p.message).collect()
}

// --- validation ------------------------------------------------------------

#[test]
fn names_are_refused_with_a_sentence_saying_why() {
    assert_eq!(name_problem(""), ["Give it a name."]);
    assert_eq!(name_problem("   "), ["Give it a name."]);
    assert_eq!(name_problem("a/b"), ["Names can't contain /."]);
    assert_eq!(name_problem(".hidden"), ["Names can't start with a dot."]);
    assert_eq!(name_problem(".gpg-id"), ["Names can't start with a dot."]);
    assert_eq!(name_problem(" github"), ["Names can't start or end with a space."]);
    assert_eq!(name_problem("two\nlines"), ["Names can't contain line breaks or other control characters."]);
    assert_eq!(name_problem("nul\0"), ["Names can't contain line breaks or other control characters."]);
    assert_eq!(name_problem("back\\slash"), ["Names can't contain \\."]);
    assert_eq!(name_problem(&"x".repeat(300)), ["That name is too long."]);
    assert_eq!(name_problem(".."), ["Names can't start with a dot."]);
    assert_eq!(name_problem("GIT~1"), ["paw can't use this name: it names git's own directory."]);
    assert!(name_problem("github.com").is_empty());
    assert!(name_problem("Zürich Tourism").is_empty());
}

#[test]
fn folders_under_dot_directories_and_odd_folder_names_are_refused() {
    let problem = |folder: &str| {
        let mut draft = EntryDraft::new(folder);
        draft.set_name("x");
        draft.problems(None).into_iter().map(|p| (p.field, p.message)).collect::<Vec<_>>()
    };
    let folder = |message: &str| vec![(DraftField::Folder, message.to_owned())];
    assert_eq!(problem(".git"), folder("Folder names can't start with a dot."));
    assert_eq!(problem("Work/.secret"), folder("Folder names can't start with a dot."));
    assert_eq!(problem("a//b"), folder("A folder needs a name."));
    assert_eq!(problem("Work /x"), folder("Folder names can't start or end with a space."));
    assert!(problem("").is_empty());
    assert!(problem("Work/new folder").is_empty());
    // Leading and trailing slashes are only tidied away.
    let mut draft = EntryDraft::new("");
    draft.set_folder("/Work/");
    assert_eq!(draft.folder(), "Work");
}

#[test]
fn a_name_that_is_taken_by_an_entry_or_a_folder_is_refused() {
    let (_dir, tree) = tree();
    let problems = |folder: &str, name: &str| {
        let mut draft = EntryDraft::new(folder);
        draft.set_name(name);
        draft.problems(Some(&tree)).into_iter().map(|p| p.message).collect::<Vec<_>>()
    };
    assert_eq!(problems("Work", "github.com"), ["There's already an entry called 'github.com' in Work."]);
    // iPhones ignore letter case in file names, so these are the same file.
    assert_eq!(problems("Work", "GitHub.com"), ["There's already an entry called 'github.com' in Work."]);
    assert_eq!(problems("", "home wi-fi"), ["There's already an entry called 'Home Wi-Fi' at the top level."]);
    assert_eq!(problems("Work", "servers"), ["There's already a folder called 'servers' in Work."]);
    assert_eq!(problems("work", "new"), ["There's already a folder called 'Work' at the top level; use that name."]);
    assert!(problems("Work", "gitlab.com").is_empty());
    assert!(problems("Work/servers", "new").is_empty());
    assert!(problems("Brand new", "x").is_empty());
}

#[test]
fn an_edit_may_keep_its_own_name() {
    let (_dir, tree) = tree();
    let draft = edit("pw\n");
    assert!(draft.problems(Some(&tree)).is_empty());
    let mut renamed = edit("pw\n");
    renamed.set_name("Jira");
    assert_eq!(messages(&renamed.problems(Some(&tree))), ["There's already an entry called 'Jira' in Work."]);
}

#[test]
fn single_line_values_and_field_names_are_checked() {
    let mut draft = EntryDraft::new("");
    draft.set_name("x");
    draft.set_password("one\ntwo");
    draft.set_username("you\r");
    draft.set_url("otpauth://totp/x");
    draft.set_field("pin:code", "1");
    draft.set_field("", "1");
    draft.set_field("answer", "two\nlines");
    draft.set_field("multi\nline", "1");
    let problems = draft.problems(None);
    let found: Vec<(DraftField, &str)> = problems.iter().map(|p| (p.field.clone(), p.message.as_str())).collect();
    let field = |key: &str| DraftField::Field { key: key.into() };
    assert_eq!(
        found,
        [
            (DraftField::Password, "The password can't contain a line break."),
            (DraftField::Username, "The username can't contain a line break."),
            (DraftField::Url, "The website can't start with // or be a one-time-code address."),
            (field("pin:code"), "A field name can't contain a colon."),
            (field(""), "Give the field a name."),
            (field("answer"), "The value of answer can't contain a line break."),
            (field("multi\nline"), "A field name can't contain a line break."),
        ]
    );
    assert!(draft.build().is_err());
}

#[test]
fn problems_never_repeat_a_secret_value() {
    let mut draft = EntryDraft::new("");
    draft.set_name("x");
    draft.set_password("hunter2\nsecret");
    draft.set_field("pin", "4071\n9");
    for problem in draft.problems(None) {
        assert!(!problem.message.contains("hunter2") && !problem.message.contains("4071"));
    }
    assert!(!format!("{draft:?}").contains("hunter2"));
}

// --- building the text -----------------------------------------------------

#[test]
fn a_new_entry_is_laid_out_password_login_url_fields_notes() {
    let mut draft = EntryDraft::new("Work");
    draft.set_name("github.com");
    draft.set_notes("Recovery codes are in the blue notebook.\nSecond line.");
    draft.set_url("https://github.com/login");
    draft.set_field("security answer", "Biscuit");
    draft.set_username("you@example.com");
    draft.set_password("tidal-Quartz-88");
    assert_eq!(draft.path(), "Work/github.com");
    assert_eq!(
        saved(&draft),
        "tidal-Quartz-88\nlogin: you@example.com\nurl: https://github.com/login\nsecurity answer: Biscuit\n\
         Recovery codes are in the blue notebook.\nSecond line.\n"
    );
}

#[test]
fn a_new_entry_with_only_a_password_is_one_line() {
    let mut draft = EntryDraft::new("");
    draft.set_name("phone-pin");
    draft.set_password("4071");
    draft.set_username("");
    draft.set_url("");
    draft.set_notes("");
    assert_eq!(saved(&draft), "4071\n");
}

const OPENED: &str = "old-pw\r\nUser: you\r\n# not a field\r\notpauth://totp/x?secret=A\r\nurl:\thttps://a.example\r\n\
                      \r\nnote one\r\nanswer: Biscuit\r\nnote two\r\n";

#[test]
fn an_edit_changes_only_the_lines_it_touches() {
    let mut draft = edit(OPENED);
    draft.set_password("new-pw");
    assert_eq!(saved(&draft), OPENED.replacen("old-pw", "new-pw", 1));

    let mut draft = edit(OPENED);
    draft.set_username("me");
    assert_eq!(saved(&draft), OPENED.replace("User: you", "User: me"));

    let mut draft = edit(OPENED);
    draft.set_url("https://b.example");
    assert_eq!(saved(&draft), OPENED.replace("url:\thttps://a.example", "url:\thttps://b.example"));

    let mut draft = edit(OPENED);
    draft.set_field("Answer", "Rex");
    assert_eq!(saved(&draft), OPENED.replace("answer: Biscuit", "answer: Rex"));

    let mut draft = edit(OPENED);
    draft.set_field("port", "2222");
    assert_eq!(saved(&draft), format!("{OPENED}port: 2222\r\n"));

    let mut draft = edit(OPENED);
    draft.remove_field("answer");
    assert_eq!(saved(&draft), OPENED.replace("answer: Biscuit\r\n", ""));

    let mut draft = edit(OPENED);
    draft.set_username("");
    assert_eq!(saved(&draft), OPENED.replace("User: you\r\n", ""));
}

#[test]
fn setting_values_to_what_they_were_changes_nothing() {
    let doc = parse(OPENED);
    let mut draft = edit(OPENED);
    draft.set_password(doc.password());
    draft.set_username(doc.username().unwrap());
    draft.set_url(doc.url().unwrap());
    draft.set_notes(&doc.notes());
    draft.set_field("answer", "Biscuit");
    assert_eq!(saved(&draft), OPENED);
}

#[test]
fn new_notes_replace_the_old_ones_where_they_were() {
    let mut draft = edit(OPENED);
    // Note lines between the first and last non-blank note go; fields and
    // the one-time-code line in between stay where they are.
    draft.set_notes("only note\r\nnow");
    assert_eq!(
        saved(&draft),
        "old-pw\r\nUser: you\r\nonly note\r\nnow\r\notpauth://totp/x?secret=A\r\nurl:\thttps://a.example\r\n\
         answer: Biscuit\r\n"
    );
    let mut draft = edit("pw\nlogin: a\n");
    draft.set_notes("added");
    assert_eq!(saved(&draft), "pw\nlogin: a\nadded\n");
    let mut draft = edit("pw\nlogin: a\nnote\n");
    draft.set_notes("");
    assert_eq!(saved(&draft), "pw\nlogin: a\n");
}

#[test]
fn the_text_toggle_replaces_everything_and_later_changes_apply_on_top() {
    let mut draft = edit(OPENED);
    draft.set_password("dropped");
    draft.set_text("typed\nlogin: x\n");
    draft.set_url("https://x.example");
    assert_eq!(saved(&draft), "typed\nlogin: x\nurl: https://x.example\n");
}

#[test]
fn a_generated_password_is_marked_until_it_is_typed_over() {
    let mut draft = EntryDraft::new("");
    let password = draft.generate_password(&Default::default()).unwrap();
    assert!(draft.generated());
    assert_eq!(draft.build().unwrap().password(), password.as_str());
    draft.set_password("typed");
    assert!(!draft.generated());
}

// --- one-time codes ---------------------------------------------------------

const KEY_LINE: &str = "otpauth://totp/github.com?secret=JBSWY3DPEHPK3PXP";

#[test]
fn a_setup_key_adds_the_line_pass_otp_writes() {
    let mut draft = edit("pw
login: you
note
");
    draft.set_one_time_code("jbsw y3dp ehpk 3pxp");
    assert!(draft.problems(None).is_empty());
    assert_eq!(saved(&draft), format!("pw\nlogin: you\nnote\n{KEY_LINE}\n"));

    let mut draft = EntryDraft::new("Work");
    draft.set_name("github.com");
    draft.set_password("pw");
    draft.set_one_time_code("JBSWY3DPEHPK3PXP");
    assert_eq!(saved(&draft), format!("pw\n{KEY_LINE}\n"));
}

#[test]
fn a_new_key_or_address_replaces_the_line_where_it_was() {
    let pasted = "otpauth://totp/GitHub:you?secret=MZXW6YTBOI&issuer=GitHub";
    let mut draft = edit(OPENED);
    draft.set_one_time_code(pasted);
    assert_eq!(saved(&draft), OPENED.replace("otpauth://totp/x?secret=A", pasted));

    let mut draft = edit("pw\notpauth: otpauth://totp/x?secret=A\nend");
    draft.set_one_time_code("JBSWY3DPEHPK3PXP");
    assert_eq!(saved(&draft), format!("pw\notpauth: {KEY_LINE}\nend"));
}

#[test]
fn removing_the_code_removes_only_its_lines() {
    let mut draft = edit(OPENED);
    draft.remove_one_time_code();
    assert_eq!(saved(&draft), OPENED.replace("otpauth://totp/x?secret=A\r\n", ""));
    let mut draft = edit("pw\nlogin: a\notpauth://totp/x?secret=A");
    draft.remove_one_time_code();
    assert_eq!(saved(&draft), "pw\nlogin: a\n");
    // Blank text undoes the removal.
    draft.set_one_time_code("  ");
    assert_eq!(saved(&draft), "pw\nlogin: a\notpauth://totp/x?secret=A");
}

#[test]
fn a_key_that_cannot_make_codes_is_refused_without_repeating_it() {
    let cases = [
        ("JBSWY3DP1EHPK", "The setup key isn't valid."),
        ("otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP&counter=0", "Counter-based codes aren't supported."),
        ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=10", "Codes must have 6 to 8 digits."),
        ("https://github.com/settings/2fa", "This isn't a setup key or an otpauth:// address."),
    ];
    for (typed, message) in cases {
        let mut draft = edit("pw\n");
        draft.set_one_time_code(typed);
        let problems = draft.problems(None);
        assert_eq!(problems, [DraftProblem { field: DraftField::OneTimeCode, message: message.into() }], "{typed}");
        assert!(!problems[0].message.contains("JBSW"));
        assert!(draft.build().is_err());
    }
}

#[test]
fn the_text_toggle_drops_a_typed_key() {
    let mut draft = edit("pw\n");
    draft.set_one_time_code("JBSWY3DPEHPK3PXP");
    draft.set_text("typed\n");
    assert_eq!(saved(&draft), "typed\n");
}

/// The lines of a file, each with its ending. A file with no bytes is one
/// empty line, not none: an entry whose only line is an empty password
/// (no newline) is written as an empty file.
fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() { vec![""] } else { text.split_inclusive('\n').collect() }
}

#[test]
fn emptying_the_only_line_changes_that_line_and_nothing_else() {
    // Shrunk from the property test below: a one-line entry without a
    // final newline whose password is cleared.
    let mut draft = edit("0");
    draft.set_password("");
    assert_eq!(saved(&draft), "");
    assert_eq!(lines(&saved(&draft)), [""]);
    let mut draft = edit("0\n");
    draft.set_password("");
    assert_eq!(saved(&draft), "\n");
    let mut draft = edit("");
    draft.set_password("x");
    assert_eq!(saved(&draft), "x\n");
}

/// Entry-like text: fields, notes, otp lines, odd endings and spacing.
fn entry_like() -> impl Strategy<Value = String> {
    let line = prop_oneof![
        Just("login: you".to_owned()),
        Just("User:\tme".to_owned()),
        Just("url: https://example.com".to_owned()),
        Just("otpauth://totp/x?secret=A".to_owned()),
        Just("answer:  Biscuit ".to_owned()),
        Just(String::new()),
        Just("  ".to_owned()),
        Just("https://example.com/x".to_owned()),
        "[a-zA-Z0-9é€ #]{1,12}",
        "[a-z]{1,6}: [a-z0-9 ]{0,8}",
    ];
    let ending = prop_oneof![Just("\n"), Just("\r\n")];
    (
        "[ -~]{0,12}",
        proptest::collection::vec((line, ending), 0..10),
        prop_oneof![Just(""), Just("\n"), Just("\r\n")],
    )
        .prop_map(|(password, lines, last)| {
            let mut text = password;
            if !lines.is_empty() {
                text.push('\n');
            }
            for (i, (line, ending)) in lines.iter().enumerate() {
                text.push_str(line);
                if i + 1 < lines.len() {
                    text.push_str(ending);
                }
            }
            text.push_str(last);
            text
        })
}

/// One change the form can make.
#[derive(Debug, Clone)]
enum Change {
    Password(String),
    Username(String),
    Url(String),
    Field(String, String),
    /// A base32 setup key typed into the form.
    SetOtp(String),
    RemoveOtp,
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        "[ -~]{0,16}".prop_map(Change::Password),
        "[a-z@.]{1,10}".prop_map(Change::Username),
        "https://[a-z]{1,8}\\.example".prop_map(Change::Url),
        ("(answer|port|pin)", "[a-z0-9]{1,8}").prop_map(|(k, v)| Change::Field(k, v)),
        "[a-z2-7]{16}".prop_map(Change::SetOtp),
        Just(Change::RemoveOtp),
    ]
}

proptest! {
    /// Every line the change is not about keeps its bytes and its order:
    /// the result differs from the original in at most one line, plus at
    /// most one added last line (and the ending of the line before it).
    #[test]
    fn an_edit_preserves_every_untouched_line_byte_for_byte(entry in entry_like(), change in change()) {
        let mut draft = edit(&entry);
        match &change {
            Change::Password(v) => draft.set_password(v),
            Change::Username(v) => draft.set_username(v),
            Change::Url(v) => draft.set_url(v),
            Change::Field(k, v) => draft.set_field(k, v),
            Change::SetOtp(key) => draft.set_one_time_code(key),
            Change::RemoveOtp => draft.remove_one_time_code(),
        }
        let after = saved(&draft);
        let before_lines = lines(&entry);
        let after_lines = lines(&after);

        if let Change::RemoveOtp = change {
            // Exactly the one-time-code lines after the password go.
            let otp = |line: &&str| line.trim_end_matches(['\r', '\n']) == "otpauth://totp/x?secret=A";
            let kept: String = before_lines.iter().enumerate()
                .filter(|(i, line)| *i == 0 || !otp(line))
                .map(|(_, line)| *line)
                .collect();
            prop_assert_eq!(&after, &kept);
            prop_assert!(!draft.build().unwrap().has_otp());
        } else if after_lines.len() == before_lines.len() {
            let differing = before_lines.iter().zip(&after_lines).filter(|(a, b)| a != b).count();
            prop_assert!(differing <= 1, "{entry:?} -> {after:?}");
        } else {
            // An added field: everything before the last original line is
            // untouched, and the new line is last.
            prop_assert!(
                after_lines.len() == before_lines.len() + 1,
                "{:?} -> {:?}", entry, after
            );
            for (a, b) in before_lines.iter().zip(&after_lines).take(before_lines.len().saturating_sub(1)) {
                prop_assert_eq!(a, b);
            }
        }
        // And the change itself is there.
        let doc = draft.build().unwrap();
        match &change {
            Change::Password(v) => prop_assert_eq!(doc.password(), v.as_str()),
            Change::Username(v) => prop_assert_eq!(doc.username(), Some(v.as_str())),
            Change::Url(v) => prop_assert_eq!(doc.url(), Some(v.as_str())),
            Change::Field(k, v) => prop_assert_eq!(doc.field(k), Some(v.as_str())),
            Change::SetOtp(key) => {
                let made = doc.one_time_code().unwrap().unwrap();
                let typed = paw_core::otp::Totp::from_setup_key(key).unwrap();
                prop_assert_eq!(made.code_at(1_700_000_000), typed.code_at(1_700_000_000));
            }
            Change::RemoveOtp => {}
        }
    }

    /// A draft with nothing changed saves exactly the bytes it opened.
    #[test]
    fn an_untouched_edit_is_byte_for_byte_the_original(entry in entry_like()) {
        prop_assert_eq!(saved(&edit(&entry)), entry);
    }
}

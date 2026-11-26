//! Who can open an entry, read from its unencrypted headers. The entries
//! are made by GnuPG, as `pass` would make them, and by paw's own
//! encryption. Every key here is generated for the test in a throwaway
//! GnuPG home, or is the simulated card's public test key.

#![cfg(feature = "sim")]

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

mod key_common;

use paw_core::recipients::{Access, EntryRecipients, Recipient, access, entry_recipients};
use paw_core::sim::SimulatedKey;
use key_common::{PIN, decrypt_text, port_with};
use paw_core::PawError;
use paw_core::sim::fixture::encrypt;
use pgp::composed::{KeyType, SecretKeyParamsBuilder, SignedPublicSubKey};
use pgp::packet::{KeyFlags, PubKeyInner, PublicSubkey};
use pgp::ser::Serialize;
use pgp::types::{KeyDetails, Password};

const ENTRY: &str = "correct horse battery staple\nlogin: you@example.com\n";
const LAPTOP: &str = "laptop@example.com";
const BACKUP: &str = "backup@example.com";
const CARD: &str = "card@example.com";

fn card() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), false, 50)
}

fn for_card(text: &str) -> Vec<u8> {
    encrypt(&[card().decryption_key().unwrap()], text.as_bytes()).unwrap()
}

fn card_fingerprint() -> String {
    card().fingerprint_hex()
}

fn key_id(fingerprint: &str) -> Recipient {
    Recipient::KeyId { id: fingerprint[fingerprint.len() - 16..].to_owned() }
}

/// A certificate GnuPG can import for the simulated card: a fresh primary
/// key that vouches for the card's decryption key as its encryption subkey.
/// This is what the card's owner would have published.
fn card_certificate() -> Vec<u8> {
    let mut rng = rand::thread_rng();
    let primary = SecretKeyParamsBuilder::default()
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(true)
        .primary_user_id(format!("Simulated Card <{CARD}>"))
        .build()
        .unwrap()
        .generate(&mut rng)
        .unwrap();
    let mut certificate = primary.to_public_key();

    let on_card = card().decryption_key().unwrap();
    let subkey = PublicSubkey::from_inner(
        PubKeyInner::new(
            on_card.version(),
            on_card.algorithm(),
            on_card.created_at(),
            None,
            on_card.public_params().clone(),
        )
        .unwrap(),
    )
    .unwrap();
    let mut flags = KeyFlags::default();
    flags.set_encrypt_comms(true);
    flags.set_encrypt_storage(true);
    let binding = subkey
        .sign(
            &mut rng,
            &primary.primary_key,
            &certificate.primary_key,
            &Password::empty(),
            flags,
            None,
        )
        .unwrap();
    certificate.public_subkeys.push(SignedPublicSubKey::new(subkey, vec![binding]));
    certificate.to_bytes().unwrap()
}

/// A GnuPG home that exists for one test.
struct Gpg {
    home: tempfile::TempDir,
}

impl Gpg {
    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command.env("GNUPGHOME", self.home.path()).env_remove("GPG_AGENT_INFO");
        command
    }

    fn run(&self, args: &[&str], input: &[u8]) -> Vec<u8> {
        let mut child = self
            .command("gpg")
            .args(["--batch", "--quiet", "--no-tty", "--yes", "--trust-model", "always"])
            .args(["--pinentry-mode", "loopback", "--passphrase", ""])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("gpg is installed (it is in the dev shell)");
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "gpg {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    /// Fingerprint of the encryption subkey of the key called `email`:
    /// what a recipient packet names.
    fn encryption_fingerprint(&self, email: &str) -> String {
        let listing = self.run(&["--list-keys", "--with-colons", "--with-subkey-fingerprint", email], b"");
        let listing = String::from_utf8(listing).unwrap();
        let mut lines = listing.lines().skip_while(|line| !line.starts_with("sub:"));
        lines
            .find(|line| line.starts_with("fpr:"))
            .and_then(|line| line.split(':').nth(9))
            .unwrap_or_else(|| panic!("no encryption subkey for {email} in:\n{listing}"))
            .to_owned()
    }

    fn encrypt(&self, recipients: &[&str], extra: &[&str]) -> Vec<u8> {
        let mut args = vec!["--encrypt"];
        for recipient in recipients {
            args.extend(["--recipient", recipient]);
        }
        args.extend(extra);
        self.run(&args, ENTRY.as_bytes())
    }
}

impl Drop for Gpg {
    fn drop(&mut self) {
        let _ = self.command("gpgconf").args(["--kill", "all"]).output();
    }
}

/// A GnuPG home of the test's own: two generated keys, as on a laptop with
/// a backup key, and the simulated card's certificate imported.
fn gpg_home() -> Gpg {
    let gpg = Gpg { home: tempfile::tempdir().unwrap() };
    for email in [LAPTOP, BACKUP] {
        gpg.run(&["--quick-generate-key", email, "future-default", "default", "never"], b"");
    }
    gpg.run(&["--import"], &card_certificate());
    gpg
}

fn named(keys: Vec<Recipient>) -> EntryRecipients {
    EntryRecipients { keys, passphrase: false }
}

#[test]
fn an_entry_from_paw_names_the_card_and_can_be_opened() {
    let entry = for_card(ENTRY);

    assert_eq!(entry_recipients(&entry).unwrap(), named(vec![key_id(&card_fingerprint())]));
    assert_eq!(access(&entry, &card_fingerprint()), Access::CanOpen);
}

#[test]
fn the_card_reports_the_fingerprint_that_entries_name() {
    // `CardInfo.decryption_fingerprint` is what the app passes to `access`.
    let fingerprint = port_with(&card()).session().unwrap().info().decryption_fingerprint.unwrap();
    let entry = for_card(ENTRY);

    assert_eq!(access(&entry, &fingerprint), Access::CanOpen);
    // Spacing and letter case, as people paste fingerprints.
    let pasted = fingerprint.to_lowercase().as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join(" ");
    assert_eq!(access(&entry, &pasted), Access::CanOpen);
}

#[test]
fn an_entry_from_gnupg_for_the_card_can_be_opened_and_really_opens() {
    let gpg = gpg_home();
    let entry = gpg.encrypt(&[CARD], &[]);

    assert_eq!(entry_recipients(&entry).unwrap(), named(vec![key_id(&card_fingerprint())]));
    assert_eq!(access(&entry, &card_fingerprint()), Access::CanOpen);

    assert_eq!(decrypt_text(&card(), PIN, &entry), ENTRY);
}

#[test]
fn an_entry_for_another_key_is_refused_with_that_keys_id() {
    let gpg = gpg_home();
    let laptop = gpg.encryption_fingerprint(LAPTOP);
    let entry = gpg.encrypt(&[LAPTOP], &[]);

    assert_eq!(entry_recipients(&entry).unwrap(), named(vec![key_id(&laptop)]));
    assert_eq!(
        access(&entry, &card_fingerprint()),
        Access::NotForThisKey { recipients: vec![key_id(&laptop)] }
    );
    // The same entry, asked about with the key it was made for.
    assert_eq!(access(&entry, &laptop), Access::CanOpen);
}

#[test]
fn an_entry_for_two_keys_opens_with_either_and_lists_both() {
    let gpg = gpg_home();
    let laptop = gpg.encryption_fingerprint(LAPTOP);
    let backup = gpg.encryption_fingerprint(BACKUP);

    let with_card = gpg.encrypt(&[LAPTOP, CARD], &[]);
    let listed = entry_recipients(&with_card).unwrap();
    assert_eq!(listed.keys.len(), 2);
    assert!(listed.keys.contains(&key_id(&laptop)) && listed.keys.contains(&key_id(&card_fingerprint())));
    assert_eq!(access(&with_card, &card_fingerprint()), Access::CanOpen);
    assert_eq!(access(&with_card, &laptop), Access::CanOpen);

    let without_card = gpg.encrypt(&[LAPTOP, BACKUP], &[]);
    let Access::NotForThisKey { recipients } = access(&without_card, &card_fingerprint()) else {
        panic!("the card is not a recipient");
    };
    assert_eq!(recipients.len(), 2);
    assert!(recipients.contains(&key_id(&laptop)) && recipients.contains(&key_id(&backup)));
}

#[test]
fn an_entry_from_paw_for_two_keys_lists_both_in_order() {
    let on_card = card().decryption_key().unwrap();
    let other = paw_core::sim::fixture::other_key().unwrap();
    let other_fingerprint: String =
        other.fingerprint().as_bytes().iter().map(|b| format!("{b:02X}")).collect();
    let entry = encrypt(&[other, on_card], ENTRY.as_bytes()).unwrap();

    assert_eq!(
        entry_recipients(&entry).unwrap(),
        named(vec![key_id(&other_fingerprint), key_id(&card_fingerprint())])
    );
    assert_eq!(access(&entry, &card_fingerprint()), Access::CanOpen);
    assert_eq!(decrypt_text(&card(), PIN, &entry), ENTRY);
}

#[test]
fn hidden_recipients_are_reported_as_hidden() {
    let gpg = gpg_home();
    let hidden = gpg.encrypt(&[LAPTOP], &["--throw-keyids"]);
    assert_eq!(entry_recipients(&hidden).unwrap(), named(vec![Recipient::Hidden]));
    // Nothing says whose it is, so only a try can tell.
    assert_eq!(access(&hidden, &card_fingerprint()), Access::MightOpen);

    let both_hidden = gpg.encrypt(&[LAPTOP, BACKUP], &["--throw-keyids"]);
    assert_eq!(
        entry_recipients(&both_hidden).unwrap(),
        named(vec![Recipient::Hidden, Recipient::Hidden])
    );
}

#[test]
fn an_entry_locked_with_a_passphrase_is_told_apart() {
    let gpg = gpg_home();
    let symmetric = gpg.run(&["--symmetric", "--passphrase", "hunter2"], ENTRY.as_bytes());
    assert_eq!(
        entry_recipients(&symmetric).unwrap(),
        EntryRecipients { keys: vec![], passphrase: true }
    );
    let Access::Unreadable { reason } = access(&symmetric, &card_fingerprint()) else {
        panic!("a passphrase-only entry cannot be opened by a key");
    };
    assert!(reason.contains("passphrase"), "{reason}");

    // Both a key and a passphrase: the key still decides.
    let laptop = gpg.encryption_fingerprint(LAPTOP);
    let both = gpg.run(
        &["--symmetric", "--passphrase", "hunter2", "--encrypt", "--recipient", LAPTOP],
        ENTRY.as_bytes(),
    );
    assert_eq!(
        entry_recipients(&both).unwrap(),
        EntryRecipients { keys: vec![key_id(&laptop)], passphrase: true }
    );
    assert!(matches!(access(&both, &card_fingerprint()), Access::NotForThisKey { .. }));
}

/// A new-format packet with a one-byte length.
fn packet(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0xC0 | tag, body.len() as u8];
    out.extend(body);
    out
}

fn v3_recipient(id: [u8; 8]) -> Vec<u8> {
    let mut body = vec![3];
    body.extend(id);
    body.extend([18, 0xAA, 0xBB]);
    body
}

const ID: [u8; 8] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF];

#[test]
fn reads_every_way_a_packet_length_can_be_written() {
    let recipient = v3_recipient(ID);
    let expected = named(vec![Recipient::KeyId { id: "0123456789ABCDEF".into() }]);
    // The encrypted data: new format with a partial length, as GnuPG
    // streams it. Its body is never read.
    let data = [0xC0 | 18, 0xE0, 0x01];

    let len = recipient.len();
    let headers: Vec<Vec<u8>> = vec![
        // New format: one, two and five byte lengths.
        vec![0xC1, len as u8],
        vec![0xC1, 0xFF, 0, 0, 0, len as u8],
        // Old format: one, two and four byte lengths.
        vec![0x84, len as u8],
        vec![0x85, 0, len as u8],
        vec![0x86, 0, 0, 0, len as u8],
    ];
    for header in headers {
        let mut entry = header.clone();
        entry.extend(&recipient);
        entry.extend(data);
        assert_eq!(entry_recipients(&entry).unwrap(), expected, "header {header:02X?}");
    }

    // A two-byte new-format length needs a body of at least 192 bytes.
    let mut long = v3_recipient(ID);
    long.resize(300, 0);
    let mut entry = vec![0xC1, 0xC0, 108];
    entry.extend(&long);
    // Old-format data packet of indeterminate length (the old format has
    // no number for the integrity-protected kind).
    entry.extend([0x80 | (9 << 2) | 3, 0x01]);
    assert_eq!(entry_recipients(&entry).unwrap(), expected);
}

#[test]
fn reads_version_6_recipients_by_fingerprint() {
    let fingerprint = [0x5A; 32];
    let mut body = vec![6, 33, 6];
    body.extend(fingerprint);
    body.extend([25, 0xAA]);
    let mut entry = packet(1, &body);
    entry.extend(packet(1, &[6, 0, 25, 0xAA]));
    entry.extend(packet(18, &[2, 9, 2, 0]));

    let expected = "5A".repeat(32);
    assert_eq!(
        entry_recipients(&entry).unwrap(),
        named(vec![Recipient::Fingerprint { fingerprint: expected.clone() }, Recipient::Hidden])
    );
    assert_eq!(access(&entry, &expected), Access::CanOpen);
    assert_eq!(access(&entry, &card_fingerprint()), Access::MightOpen);
}

#[test]
fn a_version_6_key_is_named_by_the_first_64_bits_of_its_fingerprint() {
    let fingerprint = format!("{}{}", "0123456789ABCDEF", "5A".repeat(24));
    let mut entry = packet(1, &v3_recipient(ID));
    entry.extend(packet(18, &[2, 9, 2, 0]));
    assert_eq!(access(&entry, &fingerprint), Access::CanOpen);

    let other = format!("{}{}", "5A".repeat(24), "0123456789ABCDEF");
    assert!(matches!(access(&entry, &other), Access::NotForThisKey { .. }));
}

#[test]
fn garbage_is_unreadable_and_never_a_panic() {
    let mut cut_short = packet(1, &v3_recipient(ID));
    cut_short.truncate(6);
    let mut huge_length = vec![0xC1, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
    huge_length.extend(v3_recipient(ID));
    let mut no_data = packet(1, &v3_recipient(ID));
    no_data.extend(packet(1, &v3_recipient(ID)));
    let mut streamed_recipient = vec![0xC1, 0xE1];
    streamed_recipient.extend(v3_recipient(ID));

    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("text", b"not a gpg file".to_vec()),
        ("zeros", vec![0; 64]),
        ("one byte", vec![0xC1]),
        ("armoured", b"-----BEGIN PGP MESSAGE-----\n\nhQEMA...\n".to_vec()),
        ("cut short", cut_short),
        ("a length past the end", huge_length),
        ("recipients but no data", no_data),
        ("a recipient with no length", streamed_recipient),
        ("data with no recipient", packet(18, &[1, 2, 3])),
        ("a public key", packet(6, &[4, 0, 0, 0, 0, 22])),
        ("a literal", packet(11, b"b\0\0\0\0\0hello")),
        ("an unknown recipient version", [packet(1, &[9; 12]), packet(18, &[1])].concat()),
        ("a recipient too short for its id", [packet(1, &[3, 1, 2]), packet(18, &[1])].concat()),
        ("a v6 recipient shorter than it says", [packet(1, &[6, 40, 6, 1]), packet(18, &[1])].concat()),
        ("an empty recipient", [packet(1, &[]), packet(18, &[1])].concat()),
    ];
    for (what, bytes) in cases {
        assert!(matches!(entry_recipients(&bytes), Err(PawError::NotAnEntry(_))), "{what}");
        let Access::Unreadable { reason } = access(&bytes, &card_fingerprint()) else {
            panic!("{what} should be unreadable");
        };
        assert!(!reason.is_empty(), "{what}");
    }
}

#[test]
fn a_fingerprint_that_is_not_one_matches_nothing() {
    let entry = for_card(ENTRY);
    for fingerprint in ["", "ABCD", "not hex at all, and long enough to slice", "ü".repeat(20).as_str()] {
        assert!(
            matches!(access(&entry, fingerprint), Access::NotForThisKey { .. }),
            "{fingerprint:?}"
        );
    }
}

proptest::proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..200)) {
        let _ = access(&bytes, "0123456789ABCDEF0123456789ABCDEF01234567");
    }

    #[test]
    fn arbitrary_bytes_after_a_real_recipient_never_panic(
        tail in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..120)
    ) {
        let mut bytes = packet(1, &v3_recipient(ID));
        bytes.extend(tail);
        let _ = access(&bytes, "0123456789ABCDEF0123456789ABCDEF01234567");
    }
}

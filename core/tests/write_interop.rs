//! What paw writes, read by the tools `pass` users have: GnuPG decrypts
//! it, and git shows one entry per commit in pass's words. Every key is
//! generated for the test in a throwaway GnuPG home.

#![cfg(feature = "sim")]

mod support;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use paw_core::files::content_id;
use paw_core::entry::EntryDoc;
use paw_core::recipients::{Recipient, entry_recipients};
use paw_core::write::{EntryDraft, Keyring, WriteError, WriteStore};
use pgp::composed::{Deserializable, SignedPublicKey};
use pgp::packet::{PubKeyInner, PublicKey};
use pgp::types::KeyDetails;
use support::git;

/// A GnuPG home that exists for one test.
struct Gpg {
    home: tempfile::TempDir,
}

struct Output {
    stdout: Vec<u8>,
    stderr: String,
}

impl Gpg {
    fn new() -> Self {
        Self { home: tempfile::tempdir().unwrap() }
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command.env("GNUPGHOME", self.home.path()).env_remove("GPG_AGENT_INFO");
        command
    }

    fn try_run(&self, args: &[&str], input: &[u8]) -> (bool, Output) {
        let mut child = self
            .command("gpg")
            .args(["--batch", "--no-tty", "--yes", "--trust-model", "always"])
            .args(["--pinentry-mode", "loopback", "--passphrase", ""])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("gpg is installed (it is in the dev shell)");
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().unwrap();
        (output.status.success(), Output { stdout: output.stdout, stderr: String::from_utf8_lossy(&output.stderr).into() })
    }

    fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let (ok, output) = self.try_run(args, input);
        assert!(ok, "gpg {args:?} failed: {}", output.stderr);
        output
    }

    /// A new key for `uid` (ed25519 primary, cv25519 encryption subkey).
    /// Returns the primary fingerprint.
    fn generate(&self, uid: &str) -> String {
        self.run(&["--quick-generate-key", uid, "future-default", "default", "never"], b"");
        self.primary(uid)
    }

    fn primary(&self, who: &str) -> String {
        self.fingerprints(who)[0].clone()
    }

    /// Every fingerprint of the key `who`: primary first, then subkeys.
    fn fingerprints(&self, who: &str) -> Vec<String> {
        let listing = String::from_utf8(self.run(&["--list-keys", "--with-colons", "--with-subkey-fingerprint", who], b"").stdout).unwrap();
        listing.lines().filter(|l| l.starts_with("fpr:")).map(|l| l.split(':').nth(9).unwrap().to_owned()).collect()
    }

    fn export(&self, who: &str, armor: bool) -> Vec<u8> {
        let mut args = vec!["--export"];
        if armor {
            args.push("--armor");
        }
        args.push(who);
        self.run(&args, b"").stdout
    }

    fn encrypt(&self, recipients: &[&str], text: &[u8]) -> Vec<u8> {
        let mut args = vec!["--encrypt", "--compress-algo=none", "--no-encrypt-to"];
        for recipient in recipients {
            args.extend(["--recipient", recipient]);
        }
        self.run(&args, text).stdout
    }

    /// Decrypts and returns the text and GnuPG's status lines.
    fn decrypt(&self, ciphertext: &[u8]) -> (String, String) {
        let output = self.run(&["--status-fd", "2", "--decrypt"], ciphertext);
        (String::from_utf8(output.stdout).unwrap(), output.stderr)
    }
}

impl Drop for Gpg {
    fn drop(&mut self) {
        let _ = self.command("gpgconf").args(["--kill", "all"]).output();
    }
}

/// A store in a state directory whose top `.gpg-id` is `gpg_id`.
struct Store {
    _root: tempfile::TempDir,
    state: PathBuf,
    repo: PathBuf,
}

impl Store {
    fn new(gpg_id: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let repo = state.join(paw_core::files::layout::REPO);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(repo.join(".gpg-id"), gpg_id).unwrap();
        git(&repo, &["add", ".gpg-id"]);
        git(&repo, &["commit", "--quiet", "-m", "Set the store's key."]);
        Keyring::open(&state).unwrap().accept_all_recipients_at_setup(&paw_core::lock::StoreLock::for_repo(&repo).exclusive().unwrap(), &repo).unwrap();
        Self { _root: root, state, repo }
    }

    fn write(&self) -> WriteStore {
        WriteStore::new(&self.state, &self.repo)
    }

    fn file(&self, entry: &str) -> Vec<u8> {
        std::fs::read(self.repo.join(format!("{entry}.gpg"))).unwrap()
    }

    /// Pins the encryption subkey of `gpg`'s key `who`, as setup pins the
    /// card's key, and binds each of `lines` to it, as setup's test decrypt
    /// binds a line.
    fn pin(&self, gpg: &Gpg, who: &str, lines: &[&str]) {
        let certificate = SignedPublicKey::from_bytes(&gpg.export(who, false)[..]).unwrap();
        let subkey = &certificate.public_subkeys[0].key;
        let key = PubKeyInner::new(subkey.version(), subkey.algorithm(), subkey.created_at(), None, subkey.public_params().clone())
            .and_then(PublicKey::from_inner)
            .unwrap();
        let mut keyring = Keyring::open(&self.state).unwrap();
        let held = keyring.lock().exclusive().unwrap();
        let fingerprint = keyring.pin_card_key(&held, &key).unwrap();
        for line in lines {
            support::bind_recipient(&self.state, line, &fingerprint);
        }
    }
}

fn new_entry(folder: &str, name: &str, password: &str) -> EntryDraft {
    let mut draft = EntryDraft::new(folder);
    draft.set_name(name);
    draft.set_password(password);
    draft
}

fn key_id(fingerprint: &str) -> Recipient {
    Recipient::KeyId { id: fingerprint[24..].to_owned() }
}

#[test]
fn an_entry_saved_by_paw_decrypts_with_gpg_and_reads_as_pass_would() {
    let gpg = Gpg::new();
    let primary = gpg.generate("Alice <alice@example.com>");
    let store = Store::new(&format!("{primary}\n"));
    store.pin(&gpg, &primary, &[&primary]);

    let mut draft = new_entry("Work", "github.com", "tidal-Quartz-88");
    draft.set_username("alice@example.com");
    draft.set_url("https://github.com/login");
    draft.set_notes("Recovery codes are in the blue notebook.");
    store.write().save(&draft).unwrap();

    let (text, status) = gpg.decrypt(&store.file("Work/github.com"));
    assert_eq!(
        text,
        "tidal-Quartz-88\nlogin: alice@example.com\nurl: https://github.com/login\nRecovery codes are in the blue notebook.\n"
    );
    // What `pass show -c` copies: the first line.
    assert_eq!(text.lines().next(), Some("tidal-Quartz-88"));
    // SEIPD v1 (MDC) with AES-256 (algorithm 9), and no compression.
    assert!(status.contains("DECRYPTION_INFO 2 9"), "{status}");
    let packets = String::from_utf8(gpg.run(&["--list-packets"], &store.file("Work/github.com")).stdout).unwrap();
    assert!(!packets.contains("compressed packet"), "{packets}");
    // Encrypted to the encryption subkey, as `gpg -e -r <primary>` would.
    let subkey = &gpg.fingerprints(&primary)[1];
    assert_eq!(entry_recipients(&store.file("Work/github.com")).unwrap().keys, [key_id(subkey)]);
}

#[test]
fn an_entry_for_two_recipients_decrypts_with_each_of_them() {
    let alice = Gpg::new();
    let bob = Gpg::new();
    let alice_fpr = alice.generate("Alice <alice@example.com>");
    let bob_fpr = bob.generate("Bob <bob@example.com>");
    // One line by fingerprint, one by address, as people write them.
    let store = Store::new(&format!("{alice_fpr} # Alice's laptop\n<bob@example.com>\n"));
    store.pin(&alice, &alice_fpr, &[&alice_fpr]);
    store.pin(&bob, &bob_fpr, &["<bob@example.com>"]);

    store.write().save(&new_entry("", "shared", "for both of us")).unwrap();

    let ciphertext = store.file("shared");
    assert_eq!(alice.decrypt(&ciphertext).0, "for both of us\n");
    assert_eq!(bob.decrypt(&ciphertext).0, "for both of us\n");
    assert_eq!(
        entry_recipients(&ciphertext).unwrap().keys,
        [key_id(&alice.fingerprints(&alice_fpr)[1]), key_id(&bob.fingerprints(&bob_fpr)[1])]
    );
}

#[test]
fn a_recipient_without_a_pinned_key_stops_the_save() {
    let alice = Gpg::new();
    let alice_fpr = alice.generate("Alice <alice@example.com>");
    let store = Store::new(&format!("{alice_fpr}\nbob@example.com\n"));
    store.pin(&alice, &alice_fpr, &[&alice_fpr]);

    let error = store.write().save(&new_entry("", "shared", "pw")).unwrap_err();

    assert!(matches!(error, WriteError::MissingRecipients { missing, .. } if missing == ["bob@example.com"]));
    assert!(!store.repo.join("shared.gpg").exists());
}

#[test]
fn an_entry_written_by_gpg_and_edited_by_paw_differs_only_in_the_edited_line() {
    let gpg = Gpg::new();
    let primary = gpg.generate("Alice <alice@example.com>");
    let store = Store::new(&format!("{primary}\n"));
    store.pin(&gpg, &primary, &[&primary]);
    // As `pass insert -m` on another machine might leave it: CRLF, odd
    // spacing, a one-time-code line, no final newline.
    let original = "Old-Pass\r\nUser:   alice\r\notpauth://totp/x?secret=JBSWY3DPEHPK3PXP\r\n# keep me\r\n\
                    security answer:\tBiscuit\r\nlast line without newline";
    std::fs::create_dir_all(store.repo.join("Bank")).unwrap();
    std::fs::write(store.repo.join("Bank/acme.gpg"), gpg.encrypt(&[&primary], original.as_bytes())).unwrap();
    git(&store.repo, &["add", "Bank/acme.gpg"]);
    git(&store.repo, &["commit", "--quiet", "-m", "Add given password for Bank/acme to store."]);

    let ciphertext = store.file("Bank/acme");
    let opened = EntryDoc::parse(gpg.decrypt(&ciphertext).0.into_bytes()).unwrap();
    let mut draft = EntryDraft::edit("Bank/acme", opened, content_id(&ciphertext));
    draft.set_field("security answer", "Rex");
    store.write().save(&draft).unwrap();

    let edited = gpg.decrypt(&store.file("Bank/acme")).0;
    let before: Vec<&str> = original.split_inclusive('\n').collect();
    let after: Vec<&str> = edited.split_inclusive('\n').collect();
    assert_eq!(before.len(), after.len());
    let differing: Vec<(&&str, &&str)> = before.iter().zip(&after).filter(|(a, b)| a != b).collect();
    assert_eq!(differing, [(&"security answer:\tBiscuit\r\n", &"security answer:\tRex\r\n")]);
}

//! What paw writes, read by the tools `pass` users have: GnuPG decrypts
//! it, git shows one entry per commit in pass's words, and certificates
//! exported by GnuPG choose the key GnuPG would. Every key is generated for
//! the test in a throwaway GnuPG home.

#![cfg(feature = "sim")]

mod support;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use paw_core::copies::content_id;
use paw_core::entry::EntryDoc;
use paw_core::ffi::write::StoreWriter;
use paw_core::recipients::{Recipient, entry_recipients};
use paw_core::write::cert::{evaluate, read_certificates};
use paw_core::write::{EntryDraft, Keyring, LineStatus, WriteError, WriteStore};
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

fn now() -> u32 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as u32
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
        let repo = state.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(repo.join(".gpg-id"), gpg_id).unwrap();
        git(&repo, &["add", ".gpg-id"]);
        git(&repo, &["commit", "--quiet", "-m", "Set the store's key."]);
        Keyring::open(&state).unwrap().accept_all_recipients_at_setup(&repo).unwrap();
        Self { _root: root, state, repo }
    }

    fn write(&self) -> WriteStore {
        WriteStore::new(&self.state, &self.repo)
    }

    fn writer(&self) -> Arc<StoreWriter> {
        StoreWriter::open(self.state.to_str().unwrap().into())
    }

    fn file(&self, entry: &str) -> Vec<u8> {
        std::fs::read(self.repo.join(format!("{entry}.gpg"))).unwrap()
    }

    fn import(&self, certificate: &[u8]) {
        let mut keyring = Keyring::open(&self.state).unwrap();
        for cert in read_certificates(certificate).unwrap() {
            keyring.pin_certificate(&evaluate(&cert, now()).unwrap()).unwrap();
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
    let summaries = store.writer().import_certificate(gpg.export(&primary, true)).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].satisfies, std::slice::from_ref(&primary));
    assert_eq!(summaries[0].user_ids, ["Alice <alice@example.com>"]);

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
    store.import(&alice.export(&alice_fpr, true));
    // Bob's arrives as a binary export.
    store.import(&bob.export(&bob_fpr, false));

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
fn a_recipient_without_an_imported_key_stops_the_save() {
    let alice = Gpg::new();
    let alice_fpr = alice.generate("Alice <alice@example.com>");
    let store = Store::new(&format!("{alice_fpr}\nbob@example.com\n"));
    store.import(&alice.export(&alice_fpr, true));

    let error = store.write().save(&new_entry("", "shared", "pw")).unwrap_err();

    assert!(matches!(error, WriteError::MissingRecipients { missing, .. } if missing == ["bob@example.com"]));
    assert!(!store.repo.join("shared.gpg").exists());
}

#[test]
fn git_log_shows_one_entry_per_commit_in_pass_wording() {
    let gpg = Gpg::new();
    let primary = gpg.generate("Alice <alice@example.com>");
    let store = Store::new(&format!("{primary}\n"));
    store.import(&gpg.export(&primary, true));
    let write = store.write();

    let mut generated = EntryDraft::new("Web");
    generated.set_name("example.com");
    generated.generate_password(&Default::default()).unwrap();
    write.save(&generated).unwrap();
    write.save(&new_entry("Web", "given", "pw")).unwrap();
    let opened = EntryDoc::parse(gpg.decrypt(&store.file("Web/given")).0.into_bytes()).unwrap();
    let mut edit = EntryDraft::edit("Web/given", opened, Some(content_id(&store.file("Web/given"))));
    edit.set_username("alice");
    write.save(&edit).unwrap();
    write.rename("Web/given", "Web/renamed", None).unwrap();
    write.delete("Web/example.com").unwrap();

    let log = git(&store.repo, &["log", "--format=%s|%an <%ae>", "--name-status", "-M", "-5"]);
    assert_eq!(
        log,
        "Remove Web/example.com from store.|paw <paw@paw.invalid>\n\nD\tWeb/example.com.gpg\n\
         Rename Web/given to Web/renamed.|paw <paw@paw.invalid>\n\nR100\tWeb/given.gpg\tWeb/renamed.gpg\n\
         Edit password for Web/given using paw.|paw <paw@paw.invalid>\n\nM\tWeb/given.gpg\n\
         Add given password for Web/given to store.|paw <paw@paw.invalid>\n\nA\tWeb/given.gpg\n\
         Add generated password for Web/example.com.|paw <paw@paw.invalid>\n\nA\tWeb/example.com.gpg"
    );
    assert_eq!(gpg.decrypt(&store.file("Web/renamed")).0, "pw\nlogin: alice\n");
}

#[test]
fn an_entry_written_by_gpg_and_edited_by_paw_differs_only_in_the_edited_line() {
    let gpg = Gpg::new();
    let primary = gpg.generate("Alice <alice@example.com>");
    let store = Store::new(&format!("{primary}\n"));
    store.import(&gpg.export(&primary, true));
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
    let mut draft = EntryDraft::edit("Bank/acme", opened, Some(content_id(&ciphertext)));
    draft.set_field("security answer", "Rex");
    store.write().save(&draft).unwrap();

    let edited = gpg.decrypt(&store.file("Bank/acme")).0;
    let before: Vec<&str> = original.split_inclusive('\n').collect();
    let after: Vec<&str> = edited.split_inclusive('\n').collect();
    assert_eq!(before.len(), after.len());
    let differing: Vec<(&&str, &&str)> = before.iter().zip(&after).filter(|(a, b)| a != b).collect();
    assert_eq!(differing, [(&"security answer:\tBiscuit\r\n", &"security answer:\tRex\r\n")]);
}

/// A key whose encryption subkeys are: one that expired (made in 2020 for a
/// year), one from 2023, and one from now; plus a newer signing subkey.
fn key_with_three_encryption_subkeys(gpg: &Gpg) -> String {
    gpg.run(&["--faked-system-time", "20190101T000000", "--quick-generate-key", "Carol <carol@example.com>", "ed25519", "cert", "never"], b"");
    let primary = gpg.primary("carol@example.com");
    gpg.run(&["--faked-system-time", "20200101T000000", "--quick-add-key", &primary, "cv25519", "encr", "1y"], b"");
    gpg.run(&["--faked-system-time", "20230101T000000", "--quick-add-key", &primary, "cv25519", "encr", "never"], b"");
    gpg.run(&["--quick-add-key", &primary, "cv25519", "encr", "never"], b"");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    gpg.run(&["--quick-add-key", &primary, "ed25519", "sign", "never"], b"");
    primary
}

fn chosen(gpg: &Gpg, primary: &str) -> String {
    let certificate = read_certificates(&gpg.export(primary, true)).unwrap();
    evaluate(&certificate[0], now()).unwrap().encryption.unwrap().fingerprint
}

#[test]
fn the_newest_valid_encryption_subkey_is_chosen_as_gnupg_chooses_it() {
    let gpg = Gpg::new();
    let primary = key_with_three_encryption_subkeys(&gpg);
    let fingerprints = gpg.fingerprints(&primary);
    let (newest, middle) = (&fingerprints[3], &fingerprints[2]);
    assert_eq!(&chosen(&gpg, &primary), newest);

    // GnuPG agrees: it encrypts to the same subkey.
    let by_gpg = gpg.encrypt(&[&primary], b"x");
    assert_eq!(entry_recipients(&by_gpg).unwrap().keys, [key_id(newest)]);

    // Revoke the newest; the 2023 one is next (the 2020 one has expired).
    let script = b"key 3\nrevkey\ny\n0\n\ny\nsave\n";
    gpg.run(&["--command-fd", "0", "--status-fd", "2", "--edit-key", &primary], script);
    assert_eq!(&chosen(&gpg, &primary), middle);
    let by_gpg = gpg.encrypt(&[&primary], b"x");
    assert_eq!(entry_recipients(&by_gpg).unwrap().keys, [key_id(middle)]);

    // And paw saves for it, decryptable by GnuPG.
    let store = Store::new(&format!("{primary}\n"));
    store.import(&gpg.export(&primary, true));
    store.write().save(&new_entry("", "x", "pw")).unwrap();
    assert_eq!(entry_recipients(&store.file("x")).unwrap().keys, [key_id(middle)]);
    assert_eq!(gpg.decrypt(&store.file("x")).0, "pw\n");
}

#[test]
fn a_gpg_id_line_naming_a_revoked_or_expired_subkey_is_not_satisfied() {
    let gpg = Gpg::new();
    let primary = key_with_three_encryption_subkeys(&gpg);
    let fingerprints = gpg.fingerprints(&primary);
    let (expired, newest) = (fingerprints[1].clone(), fingerprints[3].clone());
    gpg.run(&["--command-fd", "0", "--status-fd", "2", "--edit-key", &primary], b"key 3\nrevkey\ny\n0\n\ny\nsave\n");

    let store = Store::new(&format!("{newest}\n{}\n", &expired[24..]));
    store.import(&gpg.export(&primary, true));
    let keyring = Keyring::open(&store.state).unwrap();
    assert!(matches!(keyring.line_status(&newest, now()), LineStatus::Refused { reason, .. } if reason.contains("revoked")));
    assert!(matches!(keyring.line_status(&expired[24..], now()), LineStatus::Refused { reason, .. } if reason.contains("expired")));
    let error = store.write().save(&new_entry("", "x", "pw")).unwrap_err();
    assert!(
        matches!(&error, WriteError::MissingRecipients { missing, .. }
            if missing.len() == 2 && missing[0].contains("revoked") && missing[1].contains("expired")),
        "{error:?}"
    );
    assert!(!store.repo.join("x.gpg").exists());
}

#[test]
fn a_gpg_id_line_naming_a_valid_subkey_paw_does_not_encrypt_to_needs_a_binding() {
    let gpg = Gpg::new();
    let primary = key_with_three_encryption_subkeys(&gpg);
    let fingerprints = gpg.fingerprints(&primary);
    let (middle, newest) = (fingerprints[2].clone(), fingerprints[3].clone());

    let store = Store::new(&format!("{middle}\n"));
    store.import(&gpg.export(&primary, true));
    let error = store.write().save(&new_entry("", "x", "pw")).unwrap_err();
    assert!(
        matches!(&error, WriteError::MissingRecipients { missing, .. }
            if missing.len() == 1 && missing[0].contains("not the key paw encrypts to")),
        "{error:?}"
    );
    // The import preview does not claim the line either.
    assert!(store.writer().inspect_certificate(gpg.export(&primary, true)).unwrap()[0].satisfies.is_empty());

    // The person decides the line means this certificate's chosen key.
    store.writer().bind_recipient(middle.clone(), newest.clone()).unwrap();
    store.write().save(&new_entry("", "x", "pw")).unwrap();
    assert_eq!(entry_recipients(&store.file("x")).unwrap().keys, [key_id(&newest)]);
}

#[test]
fn a_revoked_or_expired_key_and_a_private_key_are_refused() {
    let gpg = Gpg::new();
    let revoked = gpg.generate("Dan <dan@example.com>");
    // GnuPG keeps a revocation certificate for every key it makes; the
    // colon at its start is there to stop an accidental import.
    let rev = std::fs::read_to_string(gpg.home.path().join(format!("openpgp-revocs.d/{revoked}.rev"))).unwrap();
    gpg.run(&["--import"], rev.replace(":-----BEGIN", "-----BEGIN").as_bytes());
    let error = evaluate(&read_certificates(&gpg.export(&revoked, true)).unwrap()[0], now()).err().unwrap();
    assert!(error.to_string().contains("revoked"), "{error}");

    gpg.run(&["--faked-system-time", "20190101T000000", "--quick-generate-key", "Eve <eve@example.com>", "future-default", "default", "1y"], b"");
    let expired = gpg.primary("eve@example.com");
    let error = evaluate(&read_certificates(&gpg.export(&expired, true)).unwrap()[0], now()).err().unwrap();
    assert!(error.to_string().contains("expired"), "{error}");

    let secret = gpg.run(&["--export-secret-keys", "--armor", &revoked], b"").stdout;
    let error = read_certificates(&secret).err().unwrap();
    assert!(error.to_string().contains("private key"), "{error}");
    let secret = gpg.run(&["--export-secret-keys", &revoked], b"").stdout;
    assert!(read_certificates(&secret).err().unwrap().to_string().contains("private key"));

    assert!(read_certificates(b"not a key").is_err());
}

#[test]
fn certificate_import_reports_what_it_would_satisfy_before_pinning() {
    let gpg = Gpg::new();
    let primary = gpg.generate("Alice <alice@example.com>");
    let store = Store::new(&format!("{}\nsomeone@example.com\n", &primary[24..]));

    let preview = store.writer().inspect_certificate(gpg.export(&primary, true)).unwrap();
    assert_eq!(preview[0].satisfies, [primary[24..].to_owned()]);
    assert_eq!(preview[0].encryption_fingerprint.as_deref(), Some(gpg.fingerprints(&primary)[1].as_str()));
    // Inspecting pins nothing.
    assert!(Keyring::open(&store.state).unwrap().keys().iter().all(|k| k.primary.is_none()));
    assert!(!store.writer().can_save_in(String::new()));
}

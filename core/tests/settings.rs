//! What Settings reads and changes: who can open the store, Face ID copies
//! on and off, sites AutoFill filled on, the server's keys, and the text
//! Copy diagnostics makes. Every store is a clone of a bare repository in a
//! temporary directory, encrypted for the simulated card.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use paw_core::copies::{CopyCache, CopyStatus};
use paw_core::github::GitHubAccount;
use paw_core::ffi::sync::StoreSync;
use paw_core::ffi::write::StoreWriter;
use paw_core::github::{TokenVault, VaultError};
use paw_core::key::KeyPort;
use paw_core::report::{DiagnosticsFacts, LastSyncState, diagnostics_text};
use paw_core::sim::SimulatedKey;
use paw_core::sim::copies::{SimClock, SimKeystore};
use paw_core::sim::fixture::{encrypt, other_key};
use paw_core::store::pairings::{PairingFile, SitePairings};
use paw_core::write::{KeySource, Keyring, LineStatus};
use key_common::PIN;
use support::git;

fn card() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), false, 50)
}

fn for_card(text: &str) -> Vec<u8> {
    encrypt(&[card().decryption_key().unwrap()], text.as_bytes()).unwrap()
}

fn for_other(text: &str) -> Vec<u8> {
    encrypt(&[other_key().unwrap()], text.as_bytes()).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn other_fingerprint() -> String {
    use pgp::types::KeyDetails;
    hex(other_key().unwrap().fingerprint().as_bytes())
}

/// A set-up store laid out as setup leaves it (`state/repo`), with the
/// card's key pinned and the recipients accepted.
struct Store {
    _root: tempfile::TempDir,
    state: PathBuf,
}

impl Store {
    fn state(&self) -> String {
        self.state.to_string_lossy().into_owned()
    }
}

fn store_with(files: &[(&str, &[u8])]) -> Store {
    let root = tempfile::tempdir().unwrap();
    let bare = root.path().join("bare.git");
    git(root.path(), &["init", "--quiet", "--bare", "--initial-branch=main", bare.to_str().unwrap()]);
    let seed = root.path().join("seed");
    git(root.path(), &["clone", "--quiet", bare.to_str().unwrap(), seed.to_str().unwrap()]);
    for (path, contents) in files {
        let file = seed.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, contents).unwrap();
    }
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "--quiet", "-m", "The store."]);
    git(&seed, &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    let state = root.path().join("state");
    let repo = state.join("repo");
    git(root.path(), &["clone", "--quiet", bare.to_str().unwrap(), repo.to_str().unwrap()]);
    let mut keyring = Keyring::open(&state).unwrap();
    keyring.pin_card_key(&card().decryption_key().unwrap()).unwrap();
    keyring.accept_all_recipients_at_setup(&repo).unwrap();
    std::fs::write(state.join("setup"), "step=done\n").unwrap();
    Store { _root: root, state }
}

// MARK: Who can open these passwords

#[test]
fn scopes_list_each_gpg_id_and_entries_for_other_keys() {
    let card_line = format!("{}\n", card().fingerprint_hex());
    let archive_line = format!("{}\n", other_fingerprint());
    let store = store_with(&[
        (".gpg-id", card_line.as_bytes()),
        ("Banking/MyBank.gpg", &for_card("pw1\n")),
        ("Work/github.com.gpg", &for_card("pw2\n")),
        // Left over from a key the store no longer names.
        ("Old/forum.gpg", &for_other("pw3\n")),
        ("Old/shop.gpg", &for_other("pw4\n")),
        ("archive/.gpg-id", archive_line.as_bytes()),
        ("archive/laptop.gpg", &for_other("pw5\n")),
        ("archive/strays.gpg", &for_card("pw6\n")),
    ]);
    let writer = StoreWriter::open(store.state());
    let scopes = writer.recipient_scopes().unwrap();
    assert_eq!(scopes.len(), 2, "{scopes:?}");

    let top = &scopes[0];
    assert_eq!((top.folder.as_str(), top.gpg_id.as_str()), ("", ".gpg-id"));
    assert_eq!(top.entry_count, 4);
    assert_eq!(top.keys.len(), 1);
    assert!(top.keys[0].this_key);
    assert!(matches!(top.keys[0].status, LineStatus::Ready { source: KeySource::Card, .. }));
    let fpr = card().fingerprint_hex();
    assert_eq!(top.keys[0].short_id.as_deref(), Some(&fpr[24..]));
    // Every key of the top is known, so the two are certain.
    assert_eq!(top.other_key_entries, vec!["Old/forum".to_owned(), "Old/shop".to_owned()]);
    assert_eq!(top.unsure_entries, 0);

    let archive = &scopes[1];
    assert_eq!((archive.folder.as_str(), archive.gpg_id.as_str()), ("archive", "archive/.gpg-id"));
    assert_eq!(archive.entry_count, 2);
    assert!(!archive.keys[0].this_key);
    assert_eq!(archive.keys[0].status, LineStatus::Missing);
    // The entry for the named key is fine (its fingerprint is the line);
    // the one for the card may be for a subkey of the line's certificate,
    // which paw does not have: unsure, not listed.
    assert!(archive.other_key_entries.is_empty());
    assert_eq!(archive.unsure_entries, 1);
}

/// A made-up entry: the given packets, then the start of encrypted data
/// (never read).
fn invented_entry(packets: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (tag, body) in packets {
        out.extend([0xC0 | tag, body.len() as u8]);
        out.extend(body);
    }
    out.extend([0xC0 | 18, 0xE0, 0x01]);
    out
}

/// A version 3 public-key session packet for key ID `id` (16 hex digits).
fn for_key_id(id: &str) -> (u8, Vec<u8>) {
    let mut body = vec![3];
    body.extend((0..8).map(|i| u8::from_str_radix(&id[2 * i..2 * i + 2], 16).unwrap()));
    body.extend([1, 0xAA, 0xBB]);
    (1, body)
}

#[test]
fn entries_with_a_hidden_recipient_or_a_passphrase_are_unsure() {
    let fpr = card().fingerprint_hex();
    let card_id = &fpr[24..];
    let hidden = for_key_id("0000000000000000");
    let passphrase = (3, vec![4, 9, 0, 2]);
    let store = store_with(&[
        (".gpg-id", format!("{fpr}\n").as_bytes()),
        ("card.gpg", &invented_entry(&[for_key_id(card_id)])),
        ("hidden.gpg", &invented_entry(std::slice::from_ref(&hidden))),
        ("card-and-hidden.gpg", &invented_entry(&[for_key_id(card_id), hidden])),
        ("card-and-passphrase.gpg", &invented_entry(&[passphrase.clone(), for_key_id(card_id)])),
        ("passphrase.gpg", &invented_entry(&[passphrase])),
    ]);

    let scopes = StoreWriter::open(store.state()).recipient_scopes().unwrap();

    assert_eq!(scopes.len(), 1, "{scopes:?}");
    assert_eq!(scopes[0].entry_count, 5);
    assert!(scopes[0].other_key_entries.is_empty(), "{scopes:?}");
    assert_eq!(scopes[0].unsure_entries, 4, "{scopes:?}");
}

// MARK: Face ID copies on and off

fn copies_in(dir: &std::path::Path, keystore: &Arc<SimKeystore>, clock: &Arc<SimClock>) -> Arc<CopyCache> {
    CopyCache::new(dir.to_string_lossy().into_owned(), keystore.clone(), clock.clone())
}

#[test]
fn turning_copies_off_ends_them_in_both_processes_and_keeps_none() {
    let dir = tempfile::tempdir().unwrap();
    let window = dir.path().join("window");
    let keystore = SimKeystore::new();
    let clock = SimClock::new(1_790_000_000_000);
    let app = copies_in(&window, &keystore, &clock);
    let fill = copies_in(&window, &keystore, &clock);
    let eight_hours = Duration::from_secs(8 * 3600);

    assert!(app.copies_enabled(), "on by default");
    assert!(app.put("web/github.com", b"pw\n", b"id", eight_hours).unwrap().is_some());
    assert_eq!(fill.list().unwrap().len(), 1);

    app.set_copies_enabled(false).unwrap();
    assert!(!app.copies_enabled());
    assert!(!fill.copies_enabled(), "the extension reads the same folder");
    assert!(fill.list().unwrap().is_empty());
    assert_eq!(fill.status("web/github.com", b"id").unwrap(), CopyStatus::NeedsKey);
    assert!(keystore.reset_count() >= 1, "ending them resets the keys, as Lock now does");
    // Opening with the key keeps no copy while off, in either process.
    assert_eq!(fill.put("mail/fastmail", b"pw\n", b"id2", eight_hours).unwrap(), None);
    assert!(app.list().unwrap().is_empty());
    assert!(fill.open("mail/fastmail", b"id2", "Open").is_err());
    // Lock now leaves the setting as it is.
    app.lock_now().unwrap();
    assert!(!fill.copies_enabled());

    app.set_copies_enabled(true).unwrap();
    assert!(fill.copies_enabled());
    let until = fill.put("mail/fastmail", b"pw\n", b"id2", eight_hours).unwrap().unwrap();
    // Still 8 hours from this key touch, never longer.
    assert_eq!(until, 1_790_000_000_000 + 8 * 3600 * 1000);
    assert_eq!(app.list().unwrap().len(), 1);
}

#[test]
fn turning_copies_off_ends_them_even_if_the_setting_cannot_be_written() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let window = dir.path().join("window");
    let keystore = SimKeystore::new();
    let clock = SimClock::new(1_790_000_000_000);
    let app = copies_in(&window, &keystore, &clock);
    app.put("web/github.com", b"pw\n", b"id", Duration::from_secs(3600)).unwrap().unwrap();
    // As good as a full disk: nothing new can be created in the folder.
    std::fs::set_permissions(&window, std::fs::Permissions::from_mode(0o500)).unwrap();

    let result = app.set_copies_enabled(false);
    let opened = app.open("web/github.com", b"id", "Open").map(|text| text.to_vec());

    std::fs::set_permissions(&window, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err(), "the failure is reported");
    assert_eq!(keystore.reset_count(), 1, "the keys were reset anyway");
    assert!(opened.is_err(), "the copy can no longer be opened");
    assert!(app.list().unwrap().is_empty());
}

#[test]
fn turning_copies_on_wipes_a_copy_sealed_while_they_were_off() {
    let dir = tempfile::tempdir().unwrap();
    let window = dir.path().join("window");
    let keystore = SimKeystore::new();
    let clock = SimClock::new(1_790_000_000_000);
    let app = copies_in(&window, &keystore, &clock);
    app.set_copies_enabled(false).unwrap();
    // A `put` in the other process that saw "on" just before the switch
    // seals under the keys reset by it and writes its copy afterwards. The
    // same file, made in a folder that is on and moved in.
    let elsewhere = dir.path().join("elsewhere");
    let late = copies_in(&elsewhere, &keystore, &clock);
    late.put("web/github.com", b"pw\n", b"id", Duration::from_secs(3600)).unwrap().unwrap();
    for child in std::fs::read_dir(&elsewhere).unwrap() {
        let child = child.unwrap();
        std::fs::rename(child.path(), window.join(child.file_name())).unwrap();
    }

    app.set_copies_enabled(true).unwrap();

    assert!(app.list().unwrap().is_empty(), "{:?}", app.list());
    assert_eq!(app.status("web/github.com", b"id").unwrap(), CopyStatus::NeedsKey);
}

// MARK: Sites used

#[test]
fn forgetting_one_site_keeps_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let file = PairingFile::new(dir.path());
    file.record("finance/northwind", "northwind.com", None).unwrap();
    file.record("finance/northwind", "northwind.co.uk", None).unwrap();
    file.record("contoso/alice", "contoso.com", None).unwrap();
    let pairings = SitePairings::open(dir.path().to_string_lossy().into_owned());

    pairings.forget_site("finance/northwind".into(), "northwind.com".into()).unwrap();
    let left: HashMap<String, Vec<String>> = pairings.pairings().into_iter().map(|p| (p.entry, p.sites)).collect();
    assert_eq!(left["finance/northwind"], vec!["northwind.co.uk".to_owned()]);
    assert_eq!(left["contoso/alice"], vec!["contoso.com".to_owned()]);

    // The last site goes with its entry.
    pairings.forget_site("contoso/alice".into(), "contoso.com".into()).unwrap();
    assert_eq!(pairings.pairings().len(), 1);
    // Something not there is not an error.
    pairings.forget_site("nobody".into(), "example.com".into()).unwrap();
    pairings.clear_pairings(None).unwrap();
    assert!(pairings.pairings().is_empty());
}

// MARK: Server keys

#[test]
fn bundled_server_keys_are_typed_and_cannot_be_forgotten() {
    let state = tempfile::tempdir().unwrap();
    std::fs::write(state.path().join("remote"), "git@github.com:you/store.git\n").unwrap();
    let sync = StoreSync::open(state.path().to_string_lossy().into_owned()).unwrap();
    let keys = sync.server_keys().unwrap();
    assert_eq!(keys.host, "github.com");
    assert!(keys.bundled);
    let shown: Vec<(&str, &str)> = keys.keys.iter().map(|k| (k.key_type.as_str(), k.fingerprint.as_str())).collect();
    assert_eq!(
        shown,
        vec![
            ("Ed25519", "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU"),
            ("ECDSA P-256", "SHA256:p2QAMXNIC1TJYWeIOttrVc98/R1BUFWu3/LiyKgUfQM"),
            ("RSA", "SHA256:uNiVztksCsDhcc0u9e8BujQXVUpKZIDTMczCvj3tD2s"),
        ]
    );
    assert!(sync.forget_host_key().is_err());
    assert_eq!(sync.server_keys().unwrap().keys.len(), 3);
}

#[test]
fn a_confirmed_server_key_can_be_forgotten() {
    let state = tempfile::tempdir().unwrap();
    std::fs::write(state.path().join("remote"), "git@git.example.org:me/store.git\n").unwrap();
    paw_core::sync::hosts::KnownHosts::of(state.path()).add("git.example.org", 22, &support::ed25519_host_key()).unwrap();
    let sync = StoreSync::open(state.path().to_string_lossy().into_owned()).unwrap();
    let keys = sync.server_keys().unwrap();
    assert!(!keys.bundled);
    assert_eq!(keys.keys.len(), 1);
    assert_eq!(keys.keys[0].key_type, "Ed25519");
    assert!(keys.keys[0].fingerprint.starts_with("SHA256:"));

    sync.forget_host_key().unwrap();
    assert!(sync.server_keys().unwrap().keys.is_empty(), "the next connection asks again");
}

// MARK: Copy diagnostics

#[derive(Default)]
struct Vault(Mutex<HashMap<String, Vec<u8>>>);

impl TokenVault for Vault {
    fn store(&self, name: String, secret: Vec<u8>) -> Result<(), VaultError> {
        self.0.lock().unwrap().insert(name, secret);
        Ok(())
    }
    fn load(&self, name: String) -> Result<Option<Vec<u8>>, VaultError> {
        Ok(self.0.lock().unwrap().get(&name).cloned())
    }
    fn remove(&self, name: String) -> Result<(), VaultError> {
        self.0.lock().unwrap().remove(&name);
        Ok(())
    }
}

const ACCESS_TOKEN: &str = "ghu_AccessTokenNeverShown0123";
const RENEWAL_TOKEN: &str = "ghr_RenewalTokenNeverShown4567";

fn facts() -> DiagnosticsFacts {
    DiagnosticsFacts {
        app_version: "1.0".into(),
        app_build: "42".into(),
        system_version: "iOS 26.1".into(),
        device_model: "iPhone17,1".into(),
        last_sync: Some(LastSyncState::UpToDate),
        last_checked_ms: Some(1_795_520_000_000),
    }
}

#[test]
fn diagnostics_say_what_they_should_and_name_nothing() {
    let card_line = format!("{}\n", card().fingerprint_hex());
    let store = store_with(&[
        (".gpg-id", card_line.as_bytes()),
        ("Banking/MyBank-Savings.gpg", &for_card("pw1\nlogin: alice.person@example.com\n")),
        ("Banking/Cards/Visa-Gold.gpg", &for_card("pw2\n")),
        ("octo-forum.gpg", &for_card("pw3\n")),
    ]);
    std::fs::write(store.state.join("remote"), "git@github.com:octo-owner/secret-store.git\n").unwrap();

    let vault = Arc::new(Vault::default());
    vault
        .store(
            "github-session".into(),
            format!(
                "paw-github-session 1\naccess={ACCESS_TOKEN}\naccess_expires=1795548800\nrefresh={RENEWAL_TOKEN}\nrefresh_expires=1811000000\n"
            )
            .into_bytes(),
        )
        .unwrap();
    let github = GitHubAccount::open(store.state(), vault);

    let clock = SimClock::new(1_795_520_000_000);
    let port = KeyPort::new(clock.clone());
    port.set_key_log(store.state(), "app".into());
    let session = port.attach(card()).unwrap();
    let info = session.info();
    let window = store.state.join("window");
    let copies = copies_in(&window, &SimKeystore::new(), &clock);
    copies.put("Banking/MyBank-Savings", b"pw1\n", b"id", Duration::from_secs(3600)).unwrap();

    let text = diagnostics_text(facts(), Some(store.state()), port.clone(), Some(github), Some(copies.clone()));

    for line in [
        "App: 1.0 (42)",
        "System: iOS 26.1",
        "Device: iPhone17,1",
        "Store: set up",
        "Passwords: 3 in 2 folders",
        "Changes not sent: 0",
        "Last sync: up to date, checked 2026-11-24 11:33 UTC",
        "Face ID copies: on, 1 open",
        "GitHub: signed in; access until 2026-11-24 19:33 UTC; renewal until 2027-05-22 ",
        "Key: plugged in",
        "  The store's key: yes",
        "  PIN tries left: 3",
        "  Touch to open: never",
        "Key log (UTC):",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
    assert!(text.lines().count() > 20, "the key log is there:\n{text}");

    let fingerprint = card().fingerprint_hex();
    let mut never: Vec<String> = vec![
        "MyBank", "Savings", "Banking", "Cards", "Visa", "octo", "secret-store", "github.com", "alice", "example.com",
        "pw1", ACCESS_TOKEN, RENEWAL_TOKEN, "ghu_", "ghr_", PIN,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    never.push(fingerprint.clone());
    never.push(fingerprint[24..].to_owned());
    never.push(fingerprint[32..].to_owned());
    never.push(info.ident.clone());
    never.extend(info.ident.split(':').filter(|part| part.len() >= 6).map(str::to_owned));
    if let Some(ssh) = &info.authentication_fingerprint {
        never.push(ssh.clone());
        never.push(ssh[ssh.len() - 8..].to_owned());
    }
    let lower = text.to_lowercase();
    for word in never {
        assert!(!lower.contains(&word.to_lowercase()), "{word:?} is in the diagnostics:\n{text}");
    }

    // Copies off, signed out, no key, no store.
    copies.set_copies_enabled(false).unwrap();
    let bare_port = KeyPort::new(clock);
    let text = diagnostics_text(
        DiagnosticsFacts { last_sync: None, last_checked_ms: None, ..facts() },
        None,
        bare_port,
        Some(GitHubAccount::open(store.state(), Arc::new(Vault::default()))),
        Some(copies),
    );
    for line in ["Store: not set up", "Face ID copies: off", "GitHub: not signed in", "Key: not plugged in", "(empty)"] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
}

#[test]
fn the_simulator_seeds_sites_used_from_the_fixture() {
    // What `-pawSimPairings 1` records in Settings › Sites used.
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    paw_core::sim::fixture::write_fixture_store(repo.to_string_lossy().into_owned()).unwrap();
    let store = paw_core::store::PassStore::open(repo.to_string_lossy().into_owned()).unwrap();
    let pairings = SitePairings::open(dir.path().to_string_lossy().into_owned());
    assert!(pairings.record_pairing(store.clone(), "email/fastmail".into(), "https://www.fastmail.com".into()).unwrap());
    assert!(pairings.record_pairing(store, "work/Jira".into(), "https://jira.com".into()).unwrap());
    let found: Vec<(String, Vec<String>)> = pairings.pairings().into_iter().map(|p| (p.entry, p.sites)).collect();
    assert_eq!(
        found,
        vec![
            ("email/fastmail".to_owned(), vec!["fastmail.com".to_owned()]),
            ("work/Jira".to_owned(), vec!["jira.com".to_owned()]),
        ]
    );
}

#[test]
fn a_forgotten_server_key_is_checked_and_trusted_again() {
    let server = support::GitServer::start(&[]);
    let state = tempfile::tempdir().unwrap();
    std::fs::write(state.path().join("remote"), format!("{}\n", server.sim.address())).unwrap();
    let sync = StoreSync::open(state.path().to_string_lossy().into_owned()).unwrap();
    assert!(sync.server_keys().unwrap().keys.is_empty(), "nothing trusted yet");

    let presented = sync.check_server_key().unwrap();
    assert_eq!(presented.key_type, "Ed25519");
    assert_eq!(presented.fingerprint, server.sim.host_key_fingerprint());
    // A fingerprint the person did not see is not pinned.
    assert!(sync.trust_server_key("SHA256:somethingElse".into()).is_err());
    assert!(sync.server_keys().unwrap().keys.is_empty());

    sync.trust_server_key(presented.fingerprint.clone()).unwrap();
    let keys = sync.server_keys().unwrap();
    assert!(!keys.bundled);
    assert_eq!(keys.keys, vec![presented.clone()]);
    // A second trust without Forget is refused.
    assert!(sync.trust_server_key(presented.fingerprint).is_err());
    sync.forget_host_key().unwrap();
    assert!(sync.server_keys().unwrap().keys.is_empty());
}

//! Adding, editing, deleting, renaming and moving entries: the commits
//! they make, the keys they encrypt to, the "not yet pushed" queue, and
//! that no plaintext reaches the disk. Every store is a clone of a bare
//! repository in a temporary directory, encrypted for the simulated card.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use paw_core::copies::{CopyCache, CopyStatus};
use paw_core::files::content_id;
use paw_core::entry::EntryDoc;
use paw_core::entry::opened::OpenedEntry;
use paw_core::ffi::write::StoreWriter;
use paw_core::recipients::{Recipient, entry_recipients};
use paw_core::sim::SimulatedKey;
use paw_core::store::PassStore;
use paw_core::sim::copies::{SimClock, SimKeystore};
use paw_core::sync::ChangeKind;
use paw_core::sync::queue;
use paw_core::write::{
    DraftField, EntryDraft, FolderRecipients, KeySource, Keyring, LineStatus, WriteError, WriteStore,
};
use pgp::composed::{KeyType, SecretKeyParamsBuilder, SignedPublicKey, SignedPublicSubKey};
use pgp::packet::{KeyFlags, PubKeyInner, PublicKey, PublicSubkey};
use pgp::types::{KeyDetails, Password};
use key_common::{PIN, decrypt_text};
use support::{git, move_entry};

const MARKER: &str = "PLAINTEXT-MARKER-5f1c";

fn card() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), false, 50)
}

fn card_key() -> PublicKey {
    card().decryption_key().unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// A store cloned from a bare repository, its state directory around it
/// (as setup lays it out: `state/repo`), and the card's key pinned.
struct Store {
    root: tempfile::TempDir,
    state: PathBuf,
    repo: PathBuf,
}

impl Store {
    fn write(&self) -> WriteStore {
        WriteStore::new(&self.state, &self.repo)
    }

    fn keyring(&self) -> Keyring {
        Keyring::open(&self.state).unwrap()
    }

    fn git(&self, args: &[&str]) -> String {
        git(&self.repo, args)
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn file(&self, entry: &str) -> Vec<u8> {
        std::fs::read(self.repo.join(format!("{entry}.gpg"))).unwrap()
    }

    fn decrypt(&self, entry: &str) -> String {
        decrypt_text(&card(), PIN, &self.file(entry))
    }

    /// A commit from "the desktop": straight into the clone, as if pulled.
    fn desktop(&self, path: &str, contents: &[u8]) {
        let file = self.repo.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, contents).unwrap();
        self.git(&["add", path]);
        self.git(&["commit", "--quiet", "-m", &format!("Edit {path} on the desktop.")]);
    }

    fn queued(&self) -> Vec<(ChangeKind, String, Option<String>)> {
        queue::unpushed(&self.repo).unwrap().into_iter().map(|i| (i.kind, i.entry, i.previous)).collect()
    }

    fn clean(&self) {
        assert_eq!(self.git(&["status", "--porcelain"]), "");
    }

    /// `(status, path)` of each file the last commit touched, and its message.
    fn last_commit(&self) -> (Vec<String>, String) {
        let files = self.git(&["show", "--format=", "--name-status", "-M", "HEAD"]);
        (files.lines().map(str::to_owned).collect(), self.git(&["log", "-1", "--format=%s"]))
    }
}

/// A store whose top `.gpg-id` is `gpg_id`, with `files` committed.
fn store_with(gpg_id: &str, files: &[(&str, &[u8])]) -> Store {
    let root = tempfile::tempdir().unwrap();
    let bare = root.path().join("bare.git");
    git(root.path(), &["init", "--quiet", "--bare", "--initial-branch=main", bare.to_str().unwrap()]);
    let seed = root.path().join("seed");
    git(root.path(), &["clone", "--quiet", bare.to_str().unwrap(), seed.to_str().unwrap()]);
    std::fs::write(seed.join(".gpg-id"), gpg_id).unwrap();
    for (path, contents) in files {
        let file = seed.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, contents).unwrap();
    }
    git(&seed, &["add", "-A"]);
    git(&seed, &["commit", "--quiet", "-m", "The store."]);
    git(&seed, &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);

    let state = root.path().join("state");
    let repo = state.join(paw_core::files::layout::REPO);
    git(root.path(), &["clone", "--quiet", bare.to_str().unwrap(), repo.to_str().unwrap()]);
    let mut keyring = Keyring::open(&state).unwrap();
    keyring.pin_card_key(&keyring.lock().exclusive().unwrap(), &card_key()).unwrap();
    keyring.accept_all_recipients_at_setup(&keyring.lock().exclusive().unwrap(), &repo).unwrap();
    Store { root, state, repo }
}

fn encrypted(text: &str) -> Vec<u8> {
    paw_core::sim::fixture::encrypt(&[card_key()], text.as_bytes()).unwrap()
}

/// A store for the card with `Work/github.com` and `Work/Jira`.
fn store() -> Store {
    let fpr = card().fingerprint_hex();
    store_with(
        &format!("{fpr}\n"),
        &[
            ("Work/github.com.gpg", &encrypted("old-pw\nlogin: you@example.com\notpauth://totp/x?secret=A\nnote\n")),
            ("Work/Jira.gpg", &encrypted("jira-pw\n")),
        ],
    )
}

fn new_entry(folder: &str, name: &str, password: &str) -> EntryDraft {
    let mut draft = EntryDraft::new(folder);
    draft.set_name(name);
    draft.set_password(password);
    draft
}

fn open(s: &Store, entry: &str) -> EntryDraft {
    let doc = EntryDoc::parse(s.decrypt(entry).into_bytes()).unwrap();
    EntryDraft::edit(entry, doc, content_id(&s.file(entry)))
}

/// A certificate for the simulated card: a fresh primary key that binds the
/// card's decryption key as its encryption subkey, as the owner's real key
/// does. Returns it and its primary fingerprint.
fn card_certificate() -> (SignedPublicKey, String) {
    let mut rng = rand::thread_rng();
    let primary = SecretKeyParamsBuilder::default()
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(true)
        .primary_user_id("Simulated Card <card@example.com>".into())
        .build()
        .unwrap()
        .generate(&mut rng)
        .unwrap();
    let mut certificate = primary.to_public_key();
    let on_card = card_key();
    let subkey = PublicSubkey::from_inner(
        PubKeyInner::new(on_card.version(), on_card.algorithm(), on_card.created_at(), None, on_card.public_params().clone())
            .unwrap(),
    )
    .unwrap();
    let mut flags = KeyFlags::default();
    flags.set_encrypt_comms(true);
    flags.set_encrypt_storage(true);
    let binding = subkey
        .sign(&mut rng, &primary.primary_key, &certificate.primary_key, &Password::empty(), flags, None)
        .unwrap();
    certificate.public_subkeys.push(SignedPublicSubKey::new(subkey, vec![binding]));
    let fpr = hex(certificate.primary_key.fingerprint().as_bytes());
    (certificate, fpr)
}

/// Every file under `dir`, with its path.
fn all_files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for child in std::fs::read_dir(&folder).unwrap() {
            let path = child.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }
    found
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

// --- recipients ------------------------------------------------------------

#[test]
fn a_folder_whose_gpg_id_names_the_pinned_card_key_can_be_saved_in() {
    let s = store();
    let recipients = s.write().folder_recipients("Work").unwrap();
    assert!(recipients.writable);
    assert_eq!(
        recipients.lines[0].status,
        LineStatus::Ready { fingerprint: card().fingerprint_hex(), source: KeySource::Card, bound: false }
    );

    s.write().save(&new_entry("Work", "gitlab.com", "Copper!lantern7")).unwrap();

    assert_eq!(s.decrypt("Work/gitlab.com"), "Copper!lantern7\n");
    let fpr = card().fingerprint_hex();
    assert_eq!(entry_recipients(&s.file("Work/gitlab.com")).unwrap().keys, [Recipient::KeyId { id: fpr[24..].into() }]);
}

#[test]
fn saving_is_refused_naming_every_recipient_without_a_key() {
    let fpr = card().fingerprint_hex();
    let s = store_with(&format!("{fpr}\nbackup@example.com # the backup\n0x1234ABCD\nSome Name\n"), &[]);
    let head = s.head();

    let error = s.write().save(&new_entry("", "x", "pw")).unwrap_err();

    match error {
        WriteError::MissingRecipients { gpg_id, missing } => {
            assert_eq!(gpg_id, ".gpg-id");
            assert_eq!(missing, ["backup@example.com", "0x1234ABCD", "Some Name"]);
        }
        other => panic!("expected MissingRecipients, got {other:?}"),
    }
    assert_eq!(s.head(), head);
    assert!(!s.repo.join("x.gpg").exists());
    assert!(!s.write().folder_recipients("").unwrap().writable);
}

#[test]
fn a_gpg_id_naming_the_primary_key_needs_the_setup_binding_for_the_card() {
    let (_certificate, primary) = card_certificate();
    let s = store_with(&format!("{primary}\n"), &[]);

    // The card exposes only its encryption subkey; nothing on the phone
    // says it belongs to this primary key.
    assert!(matches!(
        s.write().save(&new_entry("", "x", "pw")).unwrap_err(),
        WriteError::MissingRecipients { missing, .. } if missing == [primary.clone()]
    ));

    // Setup, after the test decrypt of an entry for the card succeeded.
    let bound = s.keyring().bind_card_for_opened_entry(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), &s.repo, &card().fingerprint_hex(), "x", &encrypted("pw\n"), 0).unwrap();
    assert_eq!(bound, std::slice::from_ref(&primary));
    assert_eq!(
        s.write().folder_recipients("").unwrap().lines[0].status,
        LineStatus::Ready { fingerprint: card().fingerprint_hex(), source: KeySource::Card, bound: true }
    );

    s.write().save(&new_entry("", "x", "pw")).unwrap();
    assert_eq!(s.decrypt("x"), "pw\n");
    // The binding is kept on disk.
    assert!(matches!(s.keyring().line_status(&primary, 0), LineStatus::Ready { bound: true, .. }));
}

#[test]
fn a_binding_must_name_a_pinned_key_and_the_card_binding_needs_one_open_line() {
    let s = store_with("AAAA1111\nBBBB2222\n", &[]);
    assert!(matches!(s.keyring().bind(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), "AAAA1111", "00FF"), Err(WriteError::NotPinned { .. })));
    // Two lines no key satisfies: paw cannot tell which is the card's.
    assert!(matches!(
        s.keyring().bind_card_for_opened_entry(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), &s.repo, &card().fingerprint_hex(), "x", &encrypted("pw\n"), 0),
        Err(WriteError::MissingRecipients { missing, .. }) if missing.len() == 2
    ));
    // The person picks; then the other line is still missing.
    s.keyring().bind(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), "AAAA1111", &card().fingerprint_hex()).unwrap();
    assert!(matches!(
        s.write().save(&new_entry("", "x", "pw")).unwrap_err(),
        WriteError::MissingRecipients { missing, .. } if missing == ["BBBB2222"]
    ));
}

#[test]
fn a_certificate_binds_lines_by_fingerprint_key_id_or_email() {
    let (certificate, primary) = card_certificate();
    let info = paw_core::write::cert::evaluate(&certificate, u32::MAX / 2).unwrap();
    let subkey = card().fingerprint_hex();
    assert_eq!(info.encryption.as_ref().unwrap().fingerprint, subkey);
    assert_eq!(info.emails, ["card@example.com"]);

    for line in [
        primary.clone(),
        primary.to_lowercase(),
        format!("0x{}", &primary[24..]),
        subkey.clone(),
        format!("{}!", &subkey[24..]),
        "card@example.com".into(),
        "<Card@Example.com>".into(),
        "Simulated Card <card@example.com>".into(),
    ] {
        assert!(paw_core::write::keyring::certificate_named_by(&info, &line), "{line}");
    }
    for line in [&primary[32..], "card@example", "Simulated Card", "other@example.com"] {
        assert!(!paw_core::write::keyring::certificate_named_by(&info, line), "{line}");
    }
}

#[test]
fn two_pinned_keys_matching_one_line_are_ambiguous_until_bound() {
    let s = store_with("card@example.com\n", &[]);
    let mut keyring = s.keyring();
    let (one, _) = card_certificate();
    keyring.pin_certificate(&keyring.lock().exclusive().unwrap(), &paw_core::write::cert::evaluate(&one, 2_000_000_000).unwrap()).unwrap();
    // A second key for the same address: any key the simulated fixture has.
    let other = paw_core::sim::fixture::other_key().unwrap();
    let other_fpr = hex(other.fingerprint().as_bytes());
    let mut info = paw_core::write::cert::evaluate(&one, 2_000_000_000).unwrap();
    info.encryption.as_mut().unwrap().fingerprint = other_fpr.clone();
    info.encryption.as_mut().unwrap().key = other;
    keyring.pin_certificate(&keyring.lock().exclusive().unwrap(), &info).unwrap();

    assert!(matches!(keyring.line_status("card@example.com", 0), LineStatus::Ambiguous { fingerprints } if fingerprints.len() == 2));
    keyring.bind(&keyring.lock().exclusive().unwrap(), "card@example.com", &other_fpr).unwrap();
    assert!(matches!(keyring.line_status("card@example.com", 0), LineStatus::Ready { fingerprint, bound: true, .. } if fingerprint == other_fpr));
}

#[test]
fn a_changed_gpg_id_stops_saving_until_it_is_accepted() {
    let s = store();
    let fpr = card().fingerprint_hex();
    s.write().save(&new_entry("Work", "a", "pw")).unwrap();
    // A pull changes who can open the store: same key, named by its key ID.
    s.desktop(".gpg-id", format!("# laptop\n{}\n", &fpr[24..]).as_bytes());
    let head = s.head();

    let error = s.write().save(&new_entry("Work", "b", "pw")).unwrap_err();
    assert!(
        matches!(&error, WriteError::RecipientsChanged { gpg_id, before_gpg_id: Some(was), before, after }
            if gpg_id == ".gpg-id" && was == ".gpg-id" && *before == [fpr.clone()] && *after == [fpr[24..].to_owned()]),
        "{error:?}"
    );
    assert!(s.write().folder_recipients("Work").unwrap().changed);
    assert_eq!(s.head(), head);

    s.keyring().accept_recipients(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), &s.repo, "Work").unwrap();
    s.write().save(&new_entry("Work", "b", "pw")).unwrap();
}

/// A commit from "the desktop" that removes `path`, as if pulled.
fn desktop_remove(s: &Store, path: &str) {
    s.git(&["rm", "--quiet", path]);
    s.git(&["commit", "--quiet", "-m", &format!("Remove {path} on the desktop.")]);
}

#[test]
fn a_gpg_id_a_pull_adds_deeper_stops_saving_in_that_subtree_until_accepted() {
    let s = store();
    let fpr = card().fingerprint_hex();
    // The first save accepts nothing: setup already recorded the store.
    s.write().save(&new_entry("Work", "a", "pw")).unwrap();
    s.desktop("Work/.gpg-id", format!("{}\n", &fpr[24..]).as_bytes());

    for folder in ["Work", "Work/Team"] {
        let error = s.write().save(&new_entry(folder, "b", "pw")).unwrap_err();
        assert!(
            matches!(&error, WriteError::RecipientsChanged { gpg_id, before_gpg_id: Some(was), before, after }
                if gpg_id == "Work/.gpg-id" && was == ".gpg-id" && *before == [fpr.clone()] && *after == [fpr[24..].to_owned()]),
            "{folder}: {error:?}"
        );
        assert!(!s.write().folder_recipients(folder).unwrap().writable);
    }
    // Folders the new file does not govern are as they were.
    s.write().save(&new_entry("Personal", "b", "pw")).unwrap();
    s.write().save(&new_entry("", "c", "pw")).unwrap();

    s.keyring().accept_recipients(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), &s.repo, "Work").unwrap();
    s.write().save(&new_entry("Work", "b", "pw")).unwrap();
    s.write().save(&new_entry("Work/Team", "b", "pw")).unwrap();
    assert_eq!(s.decrypt("Work/Team/b"), "pw\n");
}

#[test]
fn a_gpg_id_a_pull_removes_stops_saving_in_that_subtree_until_accepted() {
    let fpr = card().fingerprint_hex();
    let s = store_with(&format!("{fpr}\n"), &[("Work/.gpg-id", format!("{}\n", &fpr[24..]).as_bytes())]);
    s.write().save(&new_entry("Work", "a", "pw")).unwrap();
    desktop_remove(&s, "Work/.gpg-id");

    let error = s.write().save(&new_entry("Work/Team", "b", "pw")).unwrap_err();
    assert!(
        matches!(&error, WriteError::RecipientsChanged { gpg_id, before_gpg_id: Some(was), before, after }
            if gpg_id == ".gpg-id" && was == "Work/.gpg-id" && *before == [fpr[24..].to_owned()] && *after == [fpr.clone()]),
        "{error:?}"
    );
    s.write().save(&new_entry("Personal", "b", "pw")).unwrap();

    s.keyring().accept_recipients(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), &s.repo, "Work").unwrap();
    s.write().save(&new_entry("Work/Team", "b", "pw")).unwrap();
    s.write().save(&new_entry("Work", "c", "pw")).unwrap();
    // Putting the file back is a change again.
    s.desktop("Work/.gpg-id", format!("{}\n", &fpr[24..]).as_bytes());
    assert!(matches!(s.write().save(&new_entry("Work", "d", "pw")).unwrap_err(), WriteError::RecipientsChanged { .. }));
}

#[test]
fn nothing_is_saved_before_setup_accepts_who_can_open_the_store() {
    let s = store();
    std::fs::remove_file(s.state.join(paw_core::files::layout::RECIPIENTS)).unwrap();
    s.keyring().pin_card_key(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), &card_key()).unwrap();
    let head = s.head();
    assert!(matches!(s.write().save(&new_entry("Work", "a", "pw")).unwrap_err(), WriteError::RecipientsNotConfirmed));
    assert!(!s.write().folder_recipients("Work").unwrap().writable);
    // Saving does not accept anything by itself, and accepting one folder
    // is not setup.
    assert!(matches!(s.keyring().accept_recipients(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), &s.repo, "Work").unwrap_err(), WriteError::RecipientsNotConfirmed));
    assert_eq!(s.head(), head);

    s.keyring().accept_all_recipients_at_setup(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), &s.repo).unwrap();
    s.write().save(&new_entry("Work", "a", "pw")).unwrap();
}

#[test]
fn a_subkey_the_primary_key_does_not_vouch_for_satisfies_no_line() {
    let mut rng = rand::thread_rng();
    let (mut certificate, primary_fpr) = card_certificate();
    let mut key = |name: &str| {
        SecretKeyParamsBuilder::default()
            .key_type(KeyType::Ed25519Legacy)
            .can_certify(true)
            .primary_user_id(format!("{name} <{}@example.com>", name.to_lowercase()))
            .build()
            .unwrap()
            .generate(&mut rng)
            .unwrap()
    };
    let stranger = key("Mallory");
    let loose = key("Loose").to_public_key();
    let stranger_public = stranger.to_public_key();
    let foreign = |key: &PublicKey| {
        PublicSubkey::from_inner(
            PubKeyInner::new(key.version(), key.algorithm(), key.created_at(), None, key.public_params().clone()).unwrap(),
        )
        .unwrap()
    };
    let mut flags = KeyFlags::default();
    flags.set_encrypt_comms(true);
    flags.set_encrypt_storage(true);
    // One subkey bound by someone else's primary key, one with no binding.
    let forged = foreign(&stranger_public.primary_key);
    let forged_binding = forged
        .sign(&mut rng, &stranger.primary_key, &stranger_public.primary_key, &Password::empty(), flags, None)
        .unwrap();
    let forged_fpr = hex(forged.fingerprint().as_bytes());
    certificate.public_subkeys.push(SignedPublicSubKey { key: forged, signatures: vec![forged_binding] });
    let unbound = foreign(&loose.primary_key);
    let unbound_fpr = hex(unbound.fingerprint().as_bytes());
    certificate.public_subkeys.push(SignedPublicSubKey { key: unbound, signatures: Vec::new() });

    let info = paw_core::write::cert::evaluate(&certificate, 2_000_000_000).unwrap();
    assert_eq!(info.fingerprints, [primary_fpr.clone(), card().fingerprint_hex()]);
    let s = store_with(&format!("{forged_fpr}\n{}\n", &unbound_fpr[24..]), &[]);
    std::fs::remove_file(s.state.join(paw_core::files::layout::RECIPIENTS)).unwrap();
    let mut keyring = s.keyring();
    keyring.pin_certificate(&keyring.lock().exclusive().unwrap(), &info).unwrap();
    keyring.accept_all_recipients_at_setup(&keyring.lock().exclusive().unwrap(), &s.repo).unwrap();

    for line in [forged_fpr.as_str(), &unbound_fpr[24..]] {
        assert!(
            matches!(keyring.line_status(line, 0), LineStatus::Refused { reason, .. } if reason.contains("does not vouch")),
            "{line}"
        );
    }
    let error = s.write().save(&new_entry("", "a", "pw")).unwrap_err();
    assert!(
        matches!(&error, WriteError::MissingRecipients { missing, .. }
            if missing.len() == 2 && missing.iter().all(|m| m.contains("does not vouch"))),
        "{error:?}"
    );
}

#[test]
fn a_key_that_has_expired_since_it_was_pinned_is_not_used() {
    let s = store_with("card@example.com\n", &[]);
    let (certificate, _) = card_certificate();
    let mut info = paw_core::write::cert::evaluate(&certificate, 2_000_000_000).unwrap();
    info.encryption.as_mut().unwrap().expires = Some(1_000);
    let mut keyring = s.keyring();
    keyring.pin_certificate(&keyring.lock().exclusive().unwrap(), &info).unwrap();

    assert!(matches!(keyring.line_status("card@example.com", 999), LineStatus::Ready { .. }));
    assert!(matches!(keyring.line_status("card@example.com", 1_000), LineStatus::Expired { .. }));
    assert!(matches!(
        keyring.resolve(&s.repo, "", 5_000).unwrap_err(),
        WriteError::MissingRecipients { missing, .. } if missing == ["card@example.com (its key has expired)"]
    ));
}

// --- add -------------------------------------------------------------------

#[test]
fn adding_commits_one_file_with_pass_wording_as_paw() {
    let s = store();
    let mut draft = new_entry("Work", "gitlab.com", "pw");
    draft.set_username("you@example.com");
    draft.set_url("https://gitlab.com");

    let saved = s.write().save(&draft).unwrap();

    assert_eq!(saved.entry, "Work/gitlab.com");
    assert_eq!(saved.commits.len(), 1);
    assert_eq!(saved.content_id, content_id(&s.file("Work/gitlab.com")));
    assert_eq!(
        s.last_commit(),
        (vec!["A\tWork/gitlab.com.gpg".into()], "Add given password for Work/gitlab.com to store.".into())
    );
    assert_eq!(s.git(&["log", "-1", "--format=%an <%ae>|%cn <%ce>"]), "paw <paw@paw.invalid>|paw <paw@paw.invalid>");
    assert_eq!(s.decrypt("Work/gitlab.com"), "pw\nlogin: you@example.com\nurl: https://gitlab.com\n");
    s.clean();
}

#[test]
fn a_generated_password_is_committed_as_generated_and_new_folders_appear() {
    let s = store();
    let mut draft = EntryDraft::new("Bank/cards");
    draft.set_name("credit");
    let password = draft.generate_password(&Default::default()).unwrap();

    s.write().save(&draft).unwrap();

    assert_eq!(s.last_commit().1, "Add generated password for Bank/cards/credit.");
    assert_eq!(s.decrypt("Bank/cards/credit"), format!("{}\n", password.as_str()));
    s.clean();
}

#[test]
fn commits_are_by_paw() {
    let s = store();
    s.write().save(&new_entry("", "x", "pw")).unwrap();
    assert_eq!(s.git(&["log", "-1", "--format=%an <%ae>"]), "paw <paw@paw.invalid>");
}

#[test]
fn adding_over_an_existing_name_is_refused_and_changes_nothing() {
    let s = store();
    let head = s.head();
    let before = s.file("Work/github.com");

    let error = s.write().save(&new_entry("Work", "GitHub.com", "pw")).unwrap_err();

    match error {
        WriteError::Invalid { problems } => {
            assert_eq!(problems[0].field, DraftField::Name);
            assert_eq!(problems[0].message, "There's already an entry called 'github.com' in Work.");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(s.head(), head);
    assert_eq!(s.file("Work/github.com"), before);
}

#[test]
fn names_that_reach_outside_the_store_are_refused() {
    let s = store();
    let head = s.head();
    for (folder, name) in [("", ".gpg-id"), (".git", "config"), ("Work/.hidden", "x"), ("..", "x"), ("", "a/../../b")] {
        let error = s.write().save(&new_entry(folder, name, "pw")).unwrap_err();
        assert!(matches!(error, WriteError::Invalid { .. }), "{folder}/{name}: {error:?}");
    }
    for entry in [".gpg-id", "Work/.gpg-id", ".git/config"] {
        assert!(matches!(s.write().delete(entry).unwrap_err(), WriteError::Invalid { .. }), "{entry}");
        assert!(move_entry(&s.write(), "Work/github.com", entry, None).is_err(), "{entry}");
    }
    assert_eq!(s.head(), head);
    s.clean();
}

#[test]
fn a_folder_that_is_a_link_in_the_working_copy_is_refused() {
    let s = store();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), s.repo.join("Linked")).unwrap();

    let error = s.write().save(&new_entry("Linked", "x", "pw")).unwrap_err();

    assert!(error.to_string().contains("link"), "{error}");
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}

// --- edit ------------------------------------------------------------------

#[test]
fn an_edit_rewrites_only_the_changed_line_and_commits_it_as_an_edit() {
    let s = store();
    let mut draft = open(&s, "Work/github.com");
    draft.set_password("new-pw");

    let saved = s.write().save(&draft).unwrap();

    assert_eq!(saved.commits.len(), 1);
    assert_eq!(s.decrypt("Work/github.com"), "new-pw\nlogin: you@example.com\notpauth://totp/x?secret=A\nnote\n");
    assert_eq!(
        s.last_commit(),
        (vec!["M\tWork/github.com.gpg".into()], "Edit password for Work/github.com using paw.".into())
    );
    assert_eq!(saved.content_id, content_id(&s.file("Work/github.com")));
}

#[test]
fn an_unchanged_edit_commits_nothing() {
    let s = store();
    let head = s.head();
    let saved = s.write().save(&open(&s, "Work/github.com")).unwrap();
    assert!(saved.commits.is_empty());
    assert_eq!(saved.content_id, content_id(&s.file("Work/github.com")));
    assert_eq!(s.head(), head);
}

#[test]
fn an_edit_of_an_entry_that_changed_since_it_was_opened_is_refused() {
    let s = store();
    let mut draft = open(&s, "Work/github.com");
    draft.set_password("mine");
    s.desktop("Work/github.com.gpg", &encrypted("theirs\n"));
    let head = s.head();

    assert!(matches!(s.write().save(&draft).unwrap_err(), WriteError::ChangedSinceOpened { .. }));
    assert_eq!(s.head(), head);
    assert_eq!(s.decrypt("Work/github.com"), "theirs\n");
}

#[test]
fn an_edit_that_also_renames_is_a_rename_then_an_edit() {
    let s = store();
    let mut draft = open(&s, "Work/github.com");
    draft.set_name("github");
    draft.set_password("new-pw");

    let saved = s.write().save(&draft).unwrap();

    assert_eq!(saved.previous.as_deref(), Some("Work/github.com"));
    assert_eq!(saved.commits.len(), 2);
    assert_eq!(
        s.queued(),
        [
            (ChangeKind::Edited, "Work/github".into(), None),
            (ChangeKind::Renamed, "Work/github".into(), Some("Work/github.com".into())),
        ]
    );
    assert!(s.decrypt("Work/github").starts_with("new-pw\n"));
    assert!(!s.repo.join("Work/github.com.gpg").exists());
    s.clean();
}

// --- delete ----------------------------------------------------------------

#[test]
fn deleting_commits_the_removal_and_an_emptied_folder_goes() {
    let s = store();
    s.write().save(&new_entry("Old/deeper", "only", "pw")).unwrap();

    s.write().delete("Old/deeper/only").unwrap();

    assert_eq!(s.last_commit(), (vec!["D\tOld/deeper/only.gpg".into()], "Remove Old/deeper/only from store.".into()));
    assert!(!s.repo.join("Old").exists(), "the emptied folders are gone from the working copy");
    s.clean();

    s.write().delete("Work/Jira").unwrap();
    assert!(s.repo.join("Work/github.com.gpg").exists());
    assert!(matches!(s.write().delete("Work/Jira").unwrap_err(), WriteError::NoSuchEntry { .. }));
}

// --- rename and move -------------------------------------------------------

#[test]
fn a_rename_moves_the_same_encrypted_file_without_the_key() {
    let s = store();
    let blob = s.git(&["rev-parse", "HEAD:Work/github.com.gpg"]);

    let saved = move_entry(&s.write(), "Work/github.com", "Archive/github.com", None).unwrap();

    assert_eq!(saved.entry, "Archive/github.com");
    assert_eq!(s.git(&["rev-parse", "HEAD:Archive/github.com.gpg"]), blob);
    assert_eq!(
        s.last_commit(),
        (vec!["R100\tWork/github.com.gpg\tArchive/github.com.gpg".into()], "Rename Work/github.com to Archive/github.com.".into())
    );
    assert_eq!(s.queued()[0], (ChangeKind::Renamed, "Archive/github.com".into(), Some("Work/github.com".into())));
    s.clean();
}

#[test]
fn a_rename_onto_a_taken_name_is_refused() {
    let s = store();
    let head = s.head();
    assert!(matches!(move_entry(&s.write(), "Work/github.com", "Work/jira", None).unwrap_err(), WriteError::Invalid { .. }));
    assert!(matches!(move_entry(&s.write(), "Work/nothing", "Work/x", None).unwrap_err(), WriteError::NoSuchEntry { .. }));
    assert_eq!(s.head(), head);
}

/// A store whose `Shared` folder is also encrypted for the fixture's other
/// key, pinned here as an imported certificate would be.
fn store_with_shared_folder() -> (Store, String) {
    let s = store();
    let other = paw_core::sim::fixture::other_key().unwrap();
    let other_fpr = hex(other.fingerprint().as_bytes());
    s.desktop("Shared/.gpg-id", format!("{}\n{other_fpr}\n", card().fingerprint_hex()).as_bytes());
    let (certificate, _) = card_certificate();
    let mut info = paw_core::write::cert::evaluate(&certificate, 2_000_000_000).unwrap();
    info.encryption.as_mut().unwrap().fingerprint = other_fpr.clone();
    info.encryption.as_mut().unwrap().key = other;
    info.primary = other_fpr.clone();
    info.fingerprints = vec![other_fpr.clone()];
    info.emails.clear();
    s.keyring().pin_certificate(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), &info).unwrap();
    // The folder arrived after setup: the person has seen and accepted it.
    s.keyring().accept_recipients(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), &s.repo, "Shared").unwrap();
    (s, other_fpr)
}

#[test]
fn a_move_to_other_recipients_re_encrypts() {
    let (s, other_fpr) = store_with_shared_folder();

    let doc = EntryDoc::parse(s.decrypt("Work/github.com").into_bytes()).unwrap();
    let saved = move_entry(&s.write(), "Work/github.com", "Shared/github.com", Some(&doc)).unwrap();

    let keys = entry_recipients(&s.file("Shared/github.com")).unwrap().keys;
    let card_fpr = card().fingerprint_hex();
    assert_eq!(
        keys,
        [Recipient::KeyId { id: card_fpr[24..].into() }, Recipient::KeyId { id: other_fpr[24..].into() }]
    );
    assert_eq!(s.decrypt("Shared/github.com"), "old-pw\nlogin: you@example.com\notpauth://totp/x?secret=A\nnote\n");
    assert_eq!(saved.content_id, content_id(&s.file("Shared/github.com")));
    let (files, message) = s.last_commit();
    assert_eq!(message, "Rename Work/github.com to Shared/github.com.");
    assert_eq!(files.len(), 2);
    assert_eq!(s.queued()[0], (ChangeKind::Renamed, "Shared/github.com".into(), Some("Work/github.com".into())));
    s.clean();
}

#[test]
fn a_move_to_a_folder_missing_a_key_is_refused_even_when_open() {
    let s = store();
    s.desktop("Shared/.gpg-id", format!("{}\nbackup@example.com\n", card().fingerprint_hex()).as_bytes());
    let doc = EntryDoc::parse(s.decrypt("Work/github.com").into_bytes()).unwrap();
    let head = s.head();

    // Not yet accepted: that comes first.
    let error = move_entry(&s.write(), "Work/github.com", "Shared/github.com", Some(&doc)).unwrap_err();
    assert!(matches!(error, WriteError::RecipientsChanged { .. }), "{error:?}");
    s.keyring().accept_recipients(&paw_core::lock::StoreLock::for_repo(&s.repo).exclusive().unwrap(), &s.repo, "Shared").unwrap();
    let error = move_entry(&s.write(), "Work/github.com", "Shared/github.com", Some(&doc)).unwrap_err();

    assert!(matches!(error, WriteError::MissingRecipients { missing, .. } if missing == ["backup@example.com"]));
    assert_eq!(s.head(), head);
}

// --- the queue -------------------------------------------------------------

fn five_changes(s: &Store) {
    s.write().save(&new_entry("Work", "new", "pw")).unwrap();
    let mut draft = open(s, "Work/github.com");
    draft.set_password("edited");
    s.write().save(&draft).unwrap();
    s.write().delete("Work/Jira").unwrap();
    move_entry(&s.write(), "Work/new", "Work/newer", None).unwrap();
}

#[test]
fn every_change_is_in_the_queue_with_its_kind_newest_first() {
    let s = store();
    five_changes(&s);
    assert_eq!(
        s.queued(),
        [
            (ChangeKind::Renamed, "Work/newer".into(), Some("Work/new".into())),
            (ChangeKind::Deleted, "Work/Jira".into(), None),
            (ChangeKind::Edited, "Work/github.com".into(), None),
            (ChangeKind::Added, "Work/new".into(), None),
        ]
    );
    // One path per commit (two for the rename), each with pass's wording.
    let log = s.git(&["log", "--format=%s", "--name-status", "-M", "origin/main..HEAD"]);
    assert_eq!(
        log,
        "Rename Work/new to Work/newer.\n\nR100\tWork/new.gpg\tWork/newer.gpg\n\
         Remove Work/Jira from store.\n\nD\tWork/Jira.gpg\n\
         Edit password for Work/github.com using paw.\n\nM\tWork/github.com.gpg\n\
         Add given password for Work/new to store.\n\nA\tWork/new.gpg"
    );
}

#[test]
fn each_change_can_be_undone_newest_first() {
    let s = store();
    let pushed = s.head();
    five_changes(&s);

    queue::undo_last(&s.repo).unwrap();
    assert!(s.repo.join("Work/new.gpg").exists() && !s.repo.join("Work/newer.gpg").exists());
    queue::undo_last(&s.repo).unwrap();
    assert_eq!(s.decrypt("Work/Jira"), "jira-pw\n");
    queue::undo_last(&s.repo).unwrap();
    assert!(s.decrypt("Work/github.com").starts_with("old-pw\n"));
    queue::undo_last(&s.repo).unwrap();
    assert!(!s.repo.join("Work/new.gpg").exists());
    assert_eq!(s.head(), pushed);
    s.clean();
}

#[test]
fn one_change_can_be_discarded_and_everything_can_be_discarded() {
    let s = store();
    let pushed = s.head();
    five_changes(&s);
    let edit = queue::unpushed(&s.repo).unwrap()[2].id.clone();

    queue::discard(&s.repo, &edit).unwrap();
    assert!(s.decrypt("Work/github.com").starts_with("old-pw\n"));
    assert!(s.repo.join("Work/newer.gpg").exists());
    assert_eq!(s.queued().len(), 3);

    queue::discard_all(&s.repo).unwrap();
    assert_eq!(s.head(), pushed);
    assert!(s.repo.join("Work/Jira.gpg").exists());
    s.clean();
}

#[test]
fn a_move_with_re_encryption_can_be_undone() {
    let (s, _) = store_with_shared_folder();
    let doc = EntryDoc::parse(s.decrypt("Work/github.com").into_bytes()).unwrap();
    let before = s.file("Work/github.com");
    move_entry(&s.write(), "Work/github.com", "Shared/github.com", Some(&doc)).unwrap();

    queue::undo_last(&s.repo).unwrap();

    assert_eq!(s.file("Work/github.com"), before);
    assert!(!s.repo.join("Shared/github.com.gpg").exists());
    s.clean();
}

// --- no plaintext on disk --------------------------------------------------

#[test]
fn no_plaintext_reaches_the_disk_in_the_store_or_the_state_directory() {
    let s = store();
    let mut draft = new_entry("Work", "canary", &format!("{MARKER}-password"));
    draft.set_notes(&format!("{MARKER} in the notes"));
    s.write().save(&draft).unwrap();
    let mut edit = open(&s, "Work/canary");
    edit.set_field("answer", &format!("{MARKER}-answer"));
    edit.set_name("canary2");
    s.write().save(&edit).unwrap();
    let doc = EntryDoc::parse(s.decrypt("Work/canary2").into_bytes()).unwrap();
    move_entry(&s.write(), "Work/canary2", "Elsewhere/canary3", Some(&doc)).unwrap();

    // Every file under the state directory (the store, `.git`, the pinned
    // keys) and the temporary directory around it, byte for byte...
    for (path, bytes) in all_files(s.root.path()) {
        assert!(!contains(&bytes, MARKER), "plaintext in {}", path.display());
    }
    // ...and every git object, decompressed.
    let objects = std::process::Command::new("git")
        .current_dir(&s.repo)
        .args(["cat-file", "--batch-all-objects", "--batch"])
        .output()
        .unwrap();
    assert!(objects.status.success());
    assert!(!contains(&objects.stdout, MARKER));
    // The entry really holds it.
    assert!(s.decrypt("Elsewhere/canary3").contains(MARKER));
}

#[test]
fn errors_and_debug_output_carry_no_secret() {
    let s = store();
    let mut draft = open(&s, "Work/github.com");
    draft.set_password(&format!("{MARKER}\nline"));
    let error = s.write().save(&draft).unwrap_err();
    assert!(!error.to_string().contains(MARKER));
    assert!(!format!("{error:?}").contains(MARKER));
    assert!(!format!("{draft:?}").contains(MARKER));
    assert!(!format!("{draft:?}").contains("old-pw"));
}

// --- Face ID copies --------------------------------------------------------

struct Copies {
    _dir: tempfile::TempDir,
    clock: Arc<SimClock>,
    cache: Arc<CopyCache>,
}

fn copies() -> Copies {
    let dir = tempfile::tempdir().unwrap();
    let clock = SimClock::new(1_790_000_000_000);
    let cache = CopyCache::new(dir.path().join("window").to_str().unwrap().into(), SimKeystore::new(), clock.clone());
    Copies { _dir: dir, clock, cache }
}

fn writer(s: &Store) -> Arc<StoreWriter> {
    StoreWriter::open(s.state.to_str().unwrap().into())
}

/// `entry` as a key open gives it: with the content id of its file.
fn opened(s: &Store, entry: &str) -> Arc<OpenedEntry> {
    OpenedEntry::decrypted(s.decrypt(entry).into_bytes(), content_id(&s.file(entry))).unwrap()
}

#[test]
fn an_edit_is_refused_if_the_file_changed_between_opening_and_editing() {
    let s = store();
    let entry = opened(&s, "Work/github.com");
    s.desktop("Work/github.com.gpg", &encrypted("theirs\n"));

    let draft = writer(&s).edit_draft(entry, "Work/github.com".into()).unwrap();
    draft.set_password("mine".into());

    assert!(matches!(writer(&s).save(draft, None, 0).unwrap_err(), WriteError::ChangedSinceOpened { .. }));
    assert_eq!(s.decrypt("Work/github.com"), "theirs\n");
}

#[test]
fn a_save_returns_the_entry_as_saved_open() {
    let s = store();
    let draft = writer(&s).edit_draft(opened(&s, "Work/github.com"), "Work/github.com".into()).unwrap();
    draft.set_password("new-pw".into());

    let saved = writer(&s).save(draft, None, 0).unwrap();

    let entry = saved.opened.unwrap();
    assert_eq!(entry.password().as_deref(), Some("new-pw"));
    assert_eq!(entry.username().as_deref(), Some("you@example.com"));
    assert!(saved.follow_up_failures.is_empty());
    // Editing it again is guarded by the file it was saved as.
    let again = writer(&s).edit_draft(entry, "Work/github.com".into()).unwrap();
    again.set_password("newer-pw".into());
    writer(&s).save(again, None, 0).unwrap();
    assert!(s.decrypt("Work/github.com").starts_with("newer-pw\n"));
}

#[test]
fn one_field_row_is_checked_as_the_form_checks_it() {
    use paw_core::ffi::write::field_problem;
    assert_eq!(field_problem("a:b".into(), "x".into()).unwrap().message, "A field name can't contain a colon.");
    assert_eq!(
        field_problem("pin".into(), "otpauth://totp/x?secret=A".into()).unwrap().message,
        "The value of pin can't start with // or be a one-time-code address."
    );
    assert!(field_problem("pin".into(), "1234".into()).is_none());
}

#[test]
fn a_new_entry_gets_a_copy_for_the_chosen_length() {
    let s = store();
    let c = copies();
    let draft = writer(&s).new_draft("Work".into());
    draft.set_name("fresh".into());
    draft.set_password("pw".into());

    let outcome = writer(&s).save(draft, Some(c.cache.clone()), 3600).unwrap();

    assert_eq!(outcome.face_id_until_ms, Some(1_790_000_000_000 + 3_600_000));
    assert_eq!(
        c.cache.status("Work/fresh", &content_id(&s.file("Work/fresh"))).unwrap(),
        CopyStatus::Available { until_ms: 1_790_000_000_000 + 3_600_000 }
    );
    assert_eq!(*c.cache.open("Work/fresh", &outcome.content_id, "test").unwrap(), b"pw\n");
}

#[test]
fn an_edit_reseals_a_live_copy_without_lengthening_it() {
    let s = store();
    let c = copies();
    let before = s.file("Work/github.com");
    c.cache.put("Work/github.com", s.decrypt("Work/github.com").as_bytes(), &content_id(&before), std::time::Duration::from_secs(3600)).unwrap();
    c.clock.advance_ms(600_000);

    let draft = writer(&s).edit_draft(opened(&s, "Work/github.com"), "Work/github.com".into()).unwrap();
    draft.set_password("new-pw".into());
    let outcome = writer(&s).save(draft, Some(c.cache.clone()), 3600).unwrap();

    // Still ends an hour after the key touch, not an hour after the edit.
    assert_eq!(outcome.face_id_until_ms, Some(1_790_000_000_000 + 3_600_000));
    let text = c.cache.open("Work/github.com", &content_id(&s.file("Work/github.com")), "test").unwrap();
    assert!(text.starts_with(b"new-pw\n"));
}

#[test]
fn an_edit_without_a_live_copy_makes_none() {
    let s = store();
    let c = copies();
    let draft = writer(&s).edit_draft(opened(&s, "Work/github.com"), "Work/github.com".into()).unwrap();
    draft.set_password("new-pw".into());
    let outcome = writer(&s).save(draft, Some(c.cache.clone()), 3600).unwrap();
    assert_eq!(outcome.face_id_until_ms, None);
    assert!(c.cache.list().unwrap().is_empty());
}

#[test]
fn delete_wipes_the_copy() {
    let s = store();
    let c = copies();
    c.cache.put("Work/Jira", b"x", &content_id(&s.file("Work/Jira")), std::time::Duration::from_secs(3600)).unwrap();

    writer(&s).delete("Work/Jira".into(), Some(c.cache.clone())).unwrap();

    assert!(c.cache.list().unwrap().is_empty());
}

#[test]
fn a_move_of_an_open_entry_takes_its_copy_along() {
    let (s, _) = store_with_shared_folder();
    let c = copies();
    c.cache
        .put("Work/github.com", b"old", &content_id(&s.file("Work/github.com")), std::time::Duration::from_secs(3600))
        .unwrap();

    let doc = EntryDoc::parse(s.decrypt("Work/github.com").into_bytes()).unwrap();
    let writer = s.write().with_copies(Some(c.cache.clone()), std::time::Duration::ZERO);
    let outcome = move_entry(&writer, "Work/github.com", "Shared/github.com", Some(&doc)).unwrap();

    assert_eq!(outcome.face_id_until_ms, Some(1_790_000_000_000 + 3_600_000));
    let live: Vec<String> = c.cache.list().unwrap().into_iter().map(|l| l.entry).collect();
    assert_eq!(live, ["Shared/github.com"]);
    assert!(c.cache.open("Shared/github.com", &outcome.content_id, "test").unwrap().starts_with(b"old-pw\n"));
}

#[test]
fn the_ffi_draft_reports_problems_and_a_name_already_used() {
    let s = store();
    let draft = writer(&s).new_draft("Work".into());
    assert_eq!(draft.problems(None)[0].message, "Give it a name.");
    draft.set_name("Jira".into());
    assert!(draft.problems(None).is_empty());
    // From the names already read, without the lock.
    let names = PassStore::open(s.repo.to_str().unwrap().into()).unwrap();
    assert_eq!(draft.problems(Some(names.clone()))[0].message, "There's already an entry called 'Jira' in Work.");
    draft.set_name("jira-2".into());
    assert!(draft.problems(Some(names)).is_empty());
    draft.set_name("Jira".into());
    let FolderRecipients { lines, writable, .. } = writer(&s).folder_recipients("Work".into()).unwrap();
    assert!(writable);
    assert_eq!(lines.len(), 1);
}

#[test]
fn site_pairings_follow_a_rename_a_move_and_an_edit_and_go_with_a_delete() {
    use paw_core::store::pairings::PairingFile;
    let s = store();
    let pairings = PairingFile::new(&s.state);
    let paired = || {
        let mut all = pairings.load().into_iter().map(|p| (p.entry, p.sites)).collect::<Vec<_>>();
        all.sort();
        all
    };
    let jira = || vec!["jira.com".to_owned()];
    pairings.record("Work/Jira", "jira.com", None).unwrap();
    pairings.record("Work/github.com", "github.com", None).unwrap();

    move_entry(&s.write(), "Work/Jira", "Work/jira", None).unwrap();
    assert!(paired().contains(&("Work/jira".to_owned(), jira())), "{:?}", paired());

    // An edit that moves it.
    let draft = writer(&s).edit_draft(opened(&s, "Work/jira"), "Work/jira".into()).unwrap();
    draft.set_folder("Tools".into());
    writer(&s).save(draft, None, 0).unwrap();
    assert!(paired().contains(&("Tools/jira".to_owned(), jira())), "{:?}", paired());

    // An edit that does not move it leaves the pairings alone.
    let draft = writer(&s).edit_draft(opened(&s, "Tools/jira"), "Tools/jira".into()).unwrap();
    draft.set_password("new".into());
    writer(&s).save(draft, None, 0).unwrap();
    assert!(paired().contains(&("Tools/jira".to_owned(), jira())), "{:?}", paired());

    writer(&s).delete("Tools/jira".into(), None).unwrap();
    assert_eq!(paired(), vec![("Work/github.com".to_owned(), vec!["github.com".to_owned()])]);
}

#[test]
fn one_time_code_marks_follow_saves_renames_and_deletes() {
    use paw_core::store::codes::CodeFile;
    let s = store();
    let codes = CodeFile::new(&s.state);

    // Off by default: a save with a code keeps nothing.
    let draft = writer(&s).new_draft("Work".into());
    draft.set_name("Off".into());
    draft.set_password("pw".into());
    draft.set_one_time_code("JBSWY3DPEHPK3PXP".into());
    writer(&s).save(draft, None, 0).unwrap();
    assert!(!s.state.join(paw_core::files::layout::ONE_TIME_CODES).exists());
    writer(&s).delete("Work/Off".into(), None).unwrap();
    codes.set_enabled(true).unwrap();

    let draft = writer(&s).new_draft("Work".into());
    draft.set_name("Linear".into());
    draft.set_password("pw".into());
    draft.set_one_time_code("JBSWY3DPEHPK3PXP".into());
    writer(&s).save(draft, None, 0).unwrap();
    assert_eq!(codes.load(), ["Work/Linear"]);

    let draft = writer(&s).edit_draft(opened(&s, "Work/Linear"), "Work/Linear".into()).unwrap();
    draft.set_name("linear".into());
    writer(&s).save(draft, None, 0).unwrap();
    assert_eq!(codes.load(), ["Work/linear"]);

    let draft = writer(&s).edit_draft(opened(&s, "Work/linear"), "Work/linear".into()).unwrap();
    draft.remove_one_time_code();
    writer(&s).save(draft, None, 0).unwrap();
    assert!(codes.load().is_empty(), "a code removed in the editor is unmarked");

    let draft = writer(&s).edit_draft(opened(&s, "Work/linear"), "Work/linear".into()).unwrap();
    draft.set_one_time_code("JBSWY3DPEHPK3PXP".into());
    writer(&s).save(draft, None, 0).unwrap();
    writer(&s).delete("Work/linear".into(), None).unwrap();
    assert!(codes.load().is_empty());
}

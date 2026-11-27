//! paw's own changes meeting the remote: every kind of change made with the
//! write API is replayed by integrate onto what the desktop pushed in the
//! meantime, sent with one login, and reads correctly on the desktop. Two
//! clones of one bare repository behind the in-process SSH server (see
//! `support`) stand in for the phone and the desktop.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use std::path::Path;
use std::sync::Arc;

use paw_core::sim::remote::steps;
use paw_core::files::content_id;
use paw_core::entry::EntryDoc;
use paw_core::key::KeyError;
use paw_core::sim::SimulatedKey;
use paw_core::sync::operation::SyncState;
use paw_core::task::TaskFailure;
use paw_core::sync::{ChangeKind, Connection, Integration, queue};
use paw_core::files::layout::StoreDirs;
use paw_core::write::{Draft, Keyring, WriteStore, held_back};
use pgp::types::KeyDetails;
use key_common::{PIN, decrypt, decrypt_text, signer};
use support::{GitServer, git, move_entry};

fn card() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), false, 50)
}

fn encrypted(text: &str) -> Vec<u8> {
    paw_core::sim::fixture::encrypt(&[card().decryption_key().unwrap()], text.as_bytes()).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

struct World {
    card: Arc<SimulatedKey>,
    server: GitServer,
    desktop: tempfile::TempDir,
    /// The phone's state directory; the store is in `repo/`.
    state: tempfile::TempDir,
}

impl World {
    fn repo(&self) -> std::path::PathBuf {
        self.state.path().join(paw_core::files::layout::REPO)
    }

    fn store(&self) -> StoreDirs {
        StoreDirs::new(self.state.path())
    }

    fn write(&self) -> WriteStore {
        WriteStore::open(self.state.path())
    }

    fn decrypt(&self, dir: &Path, entry: &str) -> String {
        let bytes = std::fs::read(dir.join(format!("{entry}.gpg"))).unwrap();
        decrypt_text(&self.card, PIN, &bytes)
    }

    fn desktop_push(&self, files: &[(&str, Vec<u8>)], message: &str) {
        for (path, contents) in files {
            let file = self.desktop.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, contents).unwrap();
        }
        git(self.desktop.path(), &["add", "-A"]);
        git(self.desktop.path(), &["commit", "--quiet", "-m", message]);
        git(self.desktop.path(), &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    }

    fn pull(&self) -> Integration {
        let connection = Connection::open(&self.server.remote(), &signer(&self.card)).unwrap();
        steps::fetch(&connection, &self.store()).unwrap();
        steps::integrate(&self.store()).unwrap()
    }

    fn open(&self, entry: &str) -> Draft {
        let bytes = std::fs::read(self.repo().join(format!("{entry}.gpg"))).unwrap();
        let doc = EntryDoc::parse(self.decrypt(&self.repo(), entry).into_bytes()).unwrap();
        Draft::edit(entry, doc, content_id(&bytes))
    }
}

/// A store for the card, with a `Shared` folder also for the fixture's
/// other key; cloned to the phone, whose keyring has both keys pinned.
fn world() -> World {
    let card = card();
    let server = GitServer::start(&[card.ssh_public_key()]);
    let desktop = server.desktop();
    let state = tempfile::tempdir().unwrap();
    let w = World { card, server, desktop, state };

    let card_fpr = w.card.fingerprint_hex();
    let other = paw_core::sim::fixture::other_key().unwrap();
    let other_fpr = hex(other.fingerprint().as_bytes());
    w.desktop_push(
        &[
            (".gpg-id", format!("{card_fpr}\n").into_bytes()),
            ("Shared/.gpg-id", format!("{card_fpr}\n{other_fpr}\n").into_bytes()),
            ("Work/github.com.gpg", encrypted("old-pw\nlogin: you\n")),
            ("Work/Jira.gpg", encrypted("jira\n")),
            ("Work/old.gpg", encrypted("old\n")),
            ("Work/travel.gpg", encrypted("travel\n")),
        ],
        "The store.",
    );

    let connection = Connection::open(&w.server.remote(), &signer(&w.card)).unwrap();
    steps::clone(&connection, &w.server.remote(), &w.store()).unwrap();
    let mut keyring = Keyring::open(&w.store()).unwrap();
    keyring.pin_card_key(&w.store().exclusive().unwrap(), &w.card.decryption_key().unwrap()).unwrap();
    // The fixture's other key, pinned as a key paw may encrypt to.
    keyring.pin_card_key(&w.store().exclusive().unwrap(), &other).unwrap();
    keyring.accept_all_recipients_at_setup(&w.store().exclusive().unwrap()).unwrap();
    w
}

fn new_entry(folder: &str, name: &str, password: &str) -> Draft {
    let mut draft = Draft::new(folder);
    draft.set_name(name);
    draft.set_password(password);
    draft
}

/// Add, edit, delete, rename, and move to other recipients.
fn every_kind_of_change(w: &World) {
    w.write().save(&new_entry("Work", "new", "new-pw")).unwrap();
    let mut edit = w.open("Work/github.com");
    edit.set_password("phone-pw");
    w.write().save(&edit).unwrap();
    w.write().delete("Work/Jira").unwrap();
    move_entry(&w.write(), "Work/old", "Work/older", None).unwrap();
    let travel = EntryDoc::parse(w.decrypt(&w.repo(), "Work/travel").into_bytes()).unwrap();
    move_entry(&w.write(), "Work/travel", "Shared/travel", Some(&travel)).unwrap();
}

fn queued(w: &World) -> Vec<(ChangeKind, String, Option<String>)> {
    queue::unpushed(&w.store()).unwrap().into_iter().map(|i| (i.kind, i.entry, i.previous)).collect()
}

#[test]
fn every_kind_of_change_replays_onto_a_moved_remote_and_reaches_the_desktop() {
    let w = world();
    every_kind_of_change(&w);
    w.desktop_push(&[("Bank/new.gpg", encrypted("bank\n"))], "Add Bank/new from the desktop.");

    let Integration::Integrated(report) = w.pull() else { panic!("expected an integrate") };

    assert_eq!(report.fast_forwarded, 1);
    assert_eq!(report.replayed, 5);
    assert!(report.kept_both.is_empty() && report.kept_local.is_empty() && report.dropped_deletes.is_empty());
    assert_eq!(
        queued(&w),
        [
            (ChangeKind::Renamed, "Shared/travel".into(), Some("Work/travel".into())),
            (ChangeKind::Renamed, "Work/older".into(), Some("Work/old".into())),
            (ChangeKind::Deleted, "Work/Jira".into(), None),
            (ChangeKind::Edited, "Work/github.com".into(), None),
            (ChangeKind::Added, "Work/new".into(), None),
        ]
    );
    assert_eq!(git(&w.repo(), &["status", "--porcelain"]), "");

    let report = steps::sync(&w.server.remote(), &signer(&w.card), &w.store(), true).unwrap();
    assert_eq!(report.state, SyncState::UpToDate);
    assert_eq!(report.pushed, 5);
    assert!(queued(&w).is_empty());

    // The desktop sees each change as its own commit, in pass's words.
    git(w.desktop.path(), &["pull", "--quiet", "--ff-only"]);
    // The move re-encrypted the file, so git sees a deletion and an addition
    // (or a rename of whatever similarity the random ciphertext has).
    let moved = git(w.desktop.path(), &["show", "--format=%s", "--name-status", "--no-renames", "HEAD"]);
    assert_eq!(moved, "Rename Work/travel to Shared/travel.\n\nA\tShared/travel.gpg\nD\tWork/travel.gpg");
    let log = git(w.desktop.path(), &["log", "--format=%s", "--name-status", "-M", "-4", "HEAD~1"]);
    assert_eq!(
        log,
        "Rename Work/old to Work/older.\n\nR100\tWork/old.gpg\tWork/older.gpg\n\
         Remove Work/Jira from store.\n\nD\tWork/Jira.gpg\n\
         Edit password for Work/github.com using paw.\n\nM\tWork/github.com.gpg\n\
         Add given password for Work/new to store.\n\nA\tWork/new.gpg"
    );
    let desk = w.desktop.path();
    assert_eq!(w.decrypt(desk, "Work/new"), "new-pw\n");
    assert_eq!(w.decrypt(desk, "Work/github.com"), "phone-pw\nlogin: you\n");
    assert_eq!(w.decrypt(desk, "Work/older"), "old\n");
    assert_eq!(w.decrypt(desk, "Shared/travel"), "travel\n");
    assert_eq!(w.decrypt(desk, "Bank/new"), "bank\n");
    assert!(!desk.join("Work/Jira.gpg").exists());
}

#[test]
fn a_paw_rename_of_an_entry_the_desktop_deleted_still_lands_under_its_new_name() {
    let w = world();
    move_entry(&w.write(), "Work/old", "Work/older", None).unwrap();
    git(w.desktop.path(), &["rm", "--quiet", "Work/old.gpg"]);
    w.desktop_push(&[], "Remove Work/old from store.");

    let Integration::Integrated(report) = w.pull() else { panic!("expected an integrate") };

    // The rename's delete has nothing left to remove; its new name is new.
    assert_eq!(report.replayed, 1);
    assert_eq!(w.decrypt(&w.repo(), "Work/older"), "old\n");
    assert!(!w.repo().join("Work/old.gpg").exists());
}

#[test]
fn a_waiting_change_is_not_pushed_after_its_recipients_changed() {
    let w = world();
    w.write().save(&new_entry("Work", "new", "new-pw")).unwrap();
    assert!(held_back::held_back(&w.store()).unwrap().is_empty());
    // Meanwhile the desktop adds a recipient for the whole store.
    let card_fpr = w.card.fingerprint_hex();
    let other_fpr = hex(paw_core::sim::fixture::other_key().unwrap().fingerprint().as_bytes());
    w.desktop_push(&[(".gpg-id", format!("{card_fpr}\n{other_fpr}\n").into_bytes())], "Add a recipient.");
    let desktop_tip = w.server.tip();
    let send = || steps::sync(&w.server.remote(), &signer(&w.card), &w.store(), true).unwrap();

    let report = send();

    assert_eq!(report.state, SyncState::Waiting { unpushed: 1 });
    assert_eq!(report.pushed, 0);
    assert_eq!(w.server.tip(), desktop_tip, "nothing was pushed");
    assert_eq!(report.integration.recipients_changed.len(), 1, "{report:?}");
    let held = held_back::held_back(&w.store()).unwrap();
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].entry, "Work/new");
    assert_eq!(held[0].gpg_id.as_deref(), Some(".gpg-id"));
    assert_eq!(held[0].encrypted_for, [paw_core::recipients::Recipient::KeyId { id: card_fpr[24..].to_owned() }]);
    assert_eq!(held[0].recipients, [card_fpr.clone(), other_fpr.clone()]);
    assert_eq!(held[0].list, held_back::HeldBackList::NotAccepted);

    // Accepting the new recipients does not make the old ciphertext right.
    Keyring::open(&w.store()).unwrap().accept_recipients(&w.store().exclusive().unwrap(), "Work").unwrap();
    let held = held_back::held_back(&w.store()).unwrap();
    assert!(held.len() == 1 && held[0].list == held_back::HeldBackList::Accepted, "{held:?}");
    assert_eq!(send().state, SyncState::Waiting { unpushed: 1 });
    assert_eq!(w.server.tip(), desktop_tip);

    // Saved again for both recipients, with the old change taken back: sent.
    let mut again = w.open("Work/new");
    again.set_password("newer-pw");
    w.write().save(&again).unwrap();
    queue::discard(&w.store(), &held[0].id).unwrap();
    assert!(held_back::held_back(&w.store()).unwrap().is_empty());
    let report = send();
    assert_eq!(report.state, SyncState::UpToDate);
    assert_eq!(report.pushed, 1);
    git(w.desktop.path(), &["pull", "--quiet", "--ff-only"]);
    assert_eq!(w.decrypt(w.desktop.path(), "Work/new"), "newer-pw\n");
}

/// The desktop adds the fixture's other key to the whole store.
fn add_a_recipient(w: &World) -> (String, String) {
    let card_fpr = w.card.fingerprint_hex();
    let other_fpr = hex(paw_core::sim::fixture::other_key().unwrap().fingerprint().as_bytes());
    w.desktop_push(&[(".gpg-id", format!("{card_fpr}\n{other_fpr}\n").into_bytes())], "Add a recipient.");
    w.pull();
    (card_fpr, other_fpr)
}

/// Opens a waiting ciphertext with the card, as the key task would.
fn open_with_card(
    w: &World,
    opened: &std::cell::RefCell<Vec<String>>,
) -> impl FnMut(&held_back::HeldBack, &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, held_back::OpenFailure<TaskFailure>> {
    move |item, bytes| {
        opened.borrow_mut().push(item.entry.clone());
        Ok(zeroize::Zeroizing::new(decrypt(&w.card, PIN, bytes).unwrap()))
    }
}

/// Re-encrypts the held-back changes as the app's key task does, opening
/// each with the card.
fn reencrypt(
    w: &World,
    opened: &std::cell::RefCell<Vec<String>>,
) -> Result<held_back::ReencryptReport, TaskFailure> {
    held_back::reencrypt_each(&w.store(), &mut open_with_card(w, opened), &mut |_, _| {})
}

/// Why each held-back change was left.
fn left(report: &held_back::ReencryptReport) -> Vec<held_back::LeftReason> {
    report.left.iter().map(|l| l.reason.clone()).collect()
}

fn head(dir: &Path) -> String {
    git2::Repository::open(dir).unwrap().head().unwrap().target().unwrap().to_string()
}

#[test]
fn held_back_changes_are_encrypted_again_once_the_new_recipients_are_accepted_and_then_sent() {
    let w = world();
    w.write().save(&new_entry("Work", "new", "new-pw")).unwrap();
    let mut edit = w.open("Work/new");
    edit.set_password("newer-pw");
    w.write().save(&edit).unwrap();
    w.write().delete("Work/Jira").unwrap();
    let (_, other_fpr) = add_a_recipient(&w);
    let before = queued(&w);
    assert_eq!(held_back::held_back(&w.store()).unwrap().len(), 2);
    let opened = std::cell::RefCell::new(Vec::new());

    // Not accepted yet: nothing is opened and nothing changes.
    let tip = head(&w.repo());
    let report = reencrypt(&w, &opened).unwrap();
    assert_eq!(report.reencrypted, 0);
    assert_eq!(left(&report), [held_back::LeftReason::NotAccepted, held_back::LeftReason::NotAccepted]);
    assert!(opened.borrow().is_empty());
    assert_eq!(head(&w.repo()), tip);

    Keyring::open(&w.store()).unwrap().accept_recipients(&w.store().exclusive().unwrap(), "Work").unwrap();
    let report = reencrypt(&w, &opened).unwrap();

    assert_eq!((report.reencrypted, left(&report)), (2, vec![]));
    assert_eq!(*opened.borrow(), ["Work/new", "Work/new"]);
    assert_eq!(queued(&w), before, "the same changes, in the same order");
    assert!(held_back::held_back(&w.store()).unwrap().is_empty());
    let bytes = std::fs::read(w.repo().join("Work/new.gpg")).unwrap();
    let found = paw_core::recipients::entry_recipients(&bytes).unwrap();
    assert_eq!(found.keys.len(), 2, "{found:?}");
    assert!(found.keys.iter().any(|k| matches!(k, paw_core::recipients::Recipient::KeyId { id } if id.contains(&other_fpr[24..28]))), "{found:?}");
    assert_eq!(w.decrypt(&w.repo(), "Work/new"), "newer-pw\n");

    let report = steps::sync(&w.server.remote(), &signer(&w.card), &w.store(), true).unwrap();
    assert_eq!(report.state, SyncState::UpToDate);
    assert_eq!(report.pushed, 3);
    git(w.desktop.path(), &["pull", "--quiet", "--ff-only"]);
    assert_eq!(w.decrypt(w.desktop.path(), "Work/new"), "newer-pw\n");
    assert!(!w.desktop.path().join("Work/Jira.gpg").exists());
    // Every version of it that was sent is encrypted for both keys.
    let desktop = git2::Repository::open(w.desktop.path()).unwrap();
    let mut commit = desktop.head().unwrap().peel_to_commit().unwrap();
    for _ in 0..3 {
        if let Ok(entry) = commit.tree().unwrap().get_path(Path::new("Work/new.gpg")) {
            let blob = desktop.find_blob(entry.id()).unwrap();
            assert_eq!(paw_core::recipients::entry_recipients(blob.content()).unwrap().keys.len(), 2, "{}", commit.id());
        }
        commit = commit.parent(0).unwrap();
    }
}

#[test]
fn a_held_back_change_that_cannot_be_opened_is_left_as_it_was() {
    let w = world();
    w.write().save(&new_entry("Work", "new", "new-pw")).unwrap();
    add_a_recipient(&w);
    Keyring::open(&w.store()).unwrap().accept_recipients(&w.store().exclusive().unwrap(), "Work").unwrap();
    let tip = head(&w.repo());

    let mut open = |_: &held_back::HeldBack, _: &[u8]| Err(held_back::OpenFailure::from(KeyError::TouchTimeout));
    let error = held_back::reencrypt_each(&w.store(), &mut open, &mut |_, _| {}).unwrap_err();

    assert!(matches!(error, TaskFailure::Key(KeyError::TouchTimeout)), "{error:?}");
    assert_eq!(head(&w.repo()), tip);
    assert_eq!(held_back::held_back(&w.store()).unwrap().len(), 1);
}

#[test]
fn a_held_back_change_the_key_cannot_open_is_left_with_why_and_the_run_goes_on() {
    let w = world();
    w.write().save(&new_entry("Work", "new", "new-pw")).unwrap();
    add_a_recipient(&w);
    Keyring::open(&w.store()).unwrap().accept_recipients(&w.store().exclusive().unwrap(), "Work").unwrap();
    let tip = head(&w.repo());

    for (error, reason) in [
        (KeyError::NotForThisKey { recipients: vec![] }, held_back::LeftReason::NotForThisKey),
        (KeyError::DecryptFailed { detail: "damaged".into() }, held_back::LeftReason::Unreadable),
    ] {
        let mut open = |_: &held_back::HeldBack, _: &[u8]| Err(held_back::OpenFailure::from(error.clone()));
        let report: held_back::ReencryptReport =
            held_back::reencrypt_each::<TaskFailure>(&w.store(), &mut open, &mut |_, _| {}).unwrap();

        assert_eq!((report.reencrypted, left(&report)), (0, vec![reason]));
        assert_eq!(head(&w.repo()), tip);
    }
}

/// How many keys each version of `path` in the queue is encrypted for,
/// oldest first (versions where it does not exist are skipped).
fn key_counts(w: &World, path: &str) -> Vec<usize> {
    let repo = git2::Repository::open(w.repo()).unwrap();
    let remote = repo.refname_to_id("refs/remotes/origin/main").unwrap();
    let mut walk = repo.revwalk().unwrap();
    walk.push_head().unwrap();
    walk.hide(remote).unwrap();
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::REVERSE).unwrap();
    walk.filter_map(|id| {
        let tree = repo.find_commit(id.unwrap()).unwrap().tree().unwrap();
        let entry = tree.get_path(Path::new(path)).ok()?;
        let blob = repo.find_blob(entry.id()).unwrap();
        Some(paw_core::recipients::entry_recipients(blob.content()).unwrap().keys.len())
    })
    .collect()
}

/// The desktop gives `Personal` both keys; the phone accepts them and
/// re-encrypts what is held back. A second run has nothing to do, and the
/// queue is sent.
fn personal_gets_two_keys_then_reencrypt(w: &World, expected: u32) {
    let card_fpr = w.card.fingerprint_hex();
    let other_fpr = hex(paw_core::sim::fixture::other_key().unwrap().fingerprint().as_bytes());
    w.desktop_push(&[("Personal/.gpg-id", format!("{card_fpr}\n{other_fpr}\n").into_bytes())], "Personal.");
    w.pull();
    Keyring::open(&w.store()).unwrap().accept_recipients(&w.store().exclusive().unwrap(), "Personal").unwrap();
    let opened = std::cell::RefCell::new(Vec::new());
    let before = queued(w);

    let report = reencrypt(w, &opened).unwrap();
    assert_eq!((report.reencrypted, left(&report)), (expected, vec![]));
    assert_eq!(queued(w), before, "the same changes, in the same order");
    let held = held_back::held_back(&w.store()).unwrap();
    assert!(held.is_empty(), "{held:?}");

    let tip = head(&w.repo());
    let report = reencrypt(w, &opened).unwrap();
    assert_eq!((report.reencrypted, left(&report)), (0, vec![]));
    assert_eq!(head(&w.repo()), tip, "a second run is a no-op");

    let report = steps::sync(&w.server.remote(), &signer(&w.card), &w.store(), true).unwrap();
    assert_eq!(report.state, SyncState::UpToDate);
    assert_eq!(report.pushed as usize, before.len());
}

#[test]
fn a_rename_that_kept_its_blob_is_re_encrypted_only_at_its_new_path() {
    let w = world();
    w.write().save(&new_entry("Work", "new", "new-pw")).unwrap();
    move_entry(&w.write(), "Work/new", "Personal/new", None).unwrap();

    personal_gets_two_keys_then_reencrypt(&w, 1);

    assert!(key_counts(&w, "Work/new.gpg").is_empty(), "sent");
    git(w.desktop.path(), &["pull", "--quiet", "--ff-only"]);
    let desktop = git2::Repository::open(w.desktop.path()).unwrap();
    let saved = desktop.revparse_single("HEAD~1").unwrap().peel_to_tree().unwrap();
    let blob = desktop.find_blob(saved.get_path(Path::new("Work/new.gpg")).unwrap().id()).unwrap();
    assert_eq!(paw_core::recipients::entry_recipients(blob.content()).unwrap().keys.len(), 1);
    assert_eq!(w.decrypt(w.desktop.path(), "Personal/new"), "new-pw\n");
}

#[test]
fn a_rename_chain_through_another_scope_is_re_encrypted_only_there() {
    let w = world();
    w.write().save(&new_entry("Work", "a", "a-pw")).unwrap();
    move_entry(&w.write(), "Work/a", "Personal/b", None).unwrap();
    move_entry(&w.write(), "Personal/b", "Work/c", None).unwrap();
    // Before the re-encryption: the same blob at all three paths.
    assert_eq!(key_counts(&w, "Work/a.gpg"), [1]);

    let card_fpr = w.card.fingerprint_hex();
    let other_fpr = hex(paw_core::sim::fixture::other_key().unwrap().fingerprint().as_bytes());
    w.desktop_push(&[("Personal/.gpg-id", format!("{card_fpr}\n{other_fpr}\n").into_bytes())], "Personal.");
    w.pull();
    Keyring::open(&w.store()).unwrap().accept_recipients(&w.store().exclusive().unwrap(), "Personal").unwrap();
    let opened = std::cell::RefCell::new(Vec::new());
    let report = reencrypt(&w, &opened).unwrap();

    assert_eq!(report.reencrypted, 1);
    assert_eq!(key_counts(&w, "Work/a.gpg"), [1]);
    assert_eq!(key_counts(&w, "Personal/b.gpg"), [2]);
    assert_eq!(key_counts(&w, "Work/c.gpg"), [1]);
    assert!(held_back::held_back(&w.store()).unwrap().is_empty());
    let report = steps::sync(&w.server.remote(), &signer(&w.card), &w.store(), true).unwrap();
    assert_eq!(report.state, SyncState::UpToDate);
    assert_eq!(report.pushed, 3);
}

#[test]
fn a_rename_chain_within_the_new_scope_is_re_encrypted_at_each_path() {
    let w = world();
    w.write().save(&new_entry("Work", "a", "a-pw")).unwrap();
    move_entry(&w.write(), "Work/a", "Personal/b", None).unwrap();
    move_entry(&w.write(), "Personal/b", "Personal/c", None).unwrap();

    personal_gets_two_keys_then_reencrypt(&w, 2);
}

#[test]
fn a_write_then_delete_is_re_encrypted_and_stays_deleted() {
    let w = world();
    w.write().save(&new_entry("Personal", "x", "x-pw")).unwrap();
    w.write().delete("Personal/x").unwrap();

    personal_gets_two_keys_then_reencrypt(&w, 1);

    assert!(!w.repo().join("Personal/x.gpg").exists());
}

#[test]
fn the_same_ciphertext_at_two_paths_is_re_encrypted_only_where_its_recipients_changed() {
    let w = world();
    w.write().save(&new_entry("Work", "a", "a-pw")).unwrap();
    // The same file at a second path, as a desktop `cp` would leave it.
    std::fs::create_dir_all(w.repo().join("Personal")).unwrap();
    std::fs::copy(w.repo().join("Work/a.gpg"), w.repo().join("Personal/b.gpg")).unwrap();
    git(&w.repo(), &["add", "Personal/b.gpg"]);
    git(&w.repo(), &["commit", "--quiet", "-m", "Add given password for Personal/b to store."]);

    personal_gets_two_keys_then_reencrypt(&w, 1);
}

#[test]
fn a_move_re_encrypted_for_its_new_scope_keeps_that_ciphertext_when_the_old_scope_changes() {
    let w = world();
    w.write().save(&new_entry("Work", "a", "a-pw")).unwrap();
    let doc = EntryDoc::parse(w.decrypt(&w.repo(), "Work/a").into_bytes()).unwrap();
    move_entry(&w.write(), "Work/a", "Shared/a", Some(&doc)).unwrap();
    assert_eq!(key_counts(&w, "Shared/a.gpg"), [2]);
    let shared = git(&w.repo(), &["rev-parse", "HEAD:Shared/a.gpg"]);
    // The desktop adds the other key to the whole store.
    add_a_recipient(&w);
    Keyring::open(&w.store()).unwrap().accept_recipients(&w.store().exclusive().unwrap(), "Work").unwrap();

    let opened = std::cell::RefCell::new(Vec::new());
    let report = reencrypt(&w, &opened).unwrap();

    assert_eq!((report.reencrypted, left(&report)), (1, vec![]));
    assert_eq!(*opened.borrow(), ["Work/a"]);
    assert_eq!(key_counts(&w, "Work/a.gpg"), [2]);
    assert_eq!(git(&w.repo(), &["rev-parse", "HEAD:Shared/a.gpg"]), shared, "left as it was");
    let report = reencrypt(&w, &opened).unwrap();
    assert_eq!(report.reencrypted, 0);
    let report = steps::sync(&w.server.remote(), &signer(&w.card), &w.store(), true).unwrap();
    assert_eq!(report.state, SyncState::UpToDate);
}

//! paw's own changes meeting the remote: every kind of change made with the
//! write API is replayed by integrate onto what the desktop pushed in the
//! meantime, sent with one login, and reads correctly on the desktop. Two
//! clones of one bare repository behind the in-process SSH server (see
//! `support`) stand in for the phone and the desktop.

mod support;

use std::path::Path;
use std::sync::Arc;

use paw_core::copies::content_id;
use paw_core::entry::EntryDoc;
use paw_core::sim::SimulatedKey;
use paw_core::ssh::{CardSigner, card_ssh_public_key};
use paw_core::sync::operation::{self, SyncState};
use paw_core::sync::{self, ChangeKind, Connection, Integration, KeptBoth, queue};
use paw_core::write::{EntryDraft, Keyring, WriteStore};
use paw_core::{card_decryption_key, decrypt_entry};
use pgp::types::KeyDetails;
use support::{GitServer, git};

const PIN: &str = "123456";

fn card() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), false, 50)
}

fn signer(card: &Arc<SimulatedKey>) -> CardSigner {
    CardSigner::new(card.clone(), PIN, || {}).unwrap()
}

fn encrypted(text: &str) -> Vec<u8> {
    paw_core::encrypt_to_keys(rand::thread_rng(), &[card_decryption_key(card()).unwrap()], text.as_bytes()).unwrap()
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
        self.state.path().join("repo")
    }

    fn write(&self) -> WriteStore {
        WriteStore::open(self.state.path())
    }

    fn decrypt(&self, dir: &Path, entry: &str) -> String {
        let bytes = std::fs::read(dir.join(format!("{entry}.gpg"))).unwrap();
        String::from_utf8(decrypt_entry(self.card.clone(), PIN, &bytes, &|| {}).unwrap()).unwrap()
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
        sync::fetch(&connection, &self.repo()).unwrap();
        sync::integrate(&self.repo()).unwrap()
    }

    fn open(&self, entry: &str) -> EntryDraft {
        let bytes = std::fs::read(self.repo().join(format!("{entry}.gpg"))).unwrap();
        let doc = EntryDoc::parse(self.decrypt(&self.repo(), entry).into_bytes()).unwrap();
        EntryDraft::edit(entry, doc, Some(content_id(&bytes)))
    }
}

/// A store for the card, with a `Shared` folder also for the fixture's
/// other key; cloned to the phone, whose keyring has both keys pinned.
fn world() -> World {
    let card = card();
    let server = GitServer::start(&[card_ssh_public_key(card.clone()).unwrap().authorized_key()]);
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
    sync::clone(&connection, &w.server.remote(), &w.repo()).unwrap();
    let mut keyring = Keyring::open(w.state.path()).unwrap();
    keyring.pin_card_key(&card_decryption_key(w.card.clone()).unwrap()).unwrap();
    // The fixture's other key, pinned as a key paw may encrypt to.
    keyring.pin_card_key(&other).unwrap();
    keyring.accept_all_recipients_at_setup(&w.repo()).unwrap();
    w
}

fn new_entry(folder: &str, name: &str, password: &str) -> EntryDraft {
    let mut draft = EntryDraft::new(folder);
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
    w.write().rename("Work/old", "Work/older", None).unwrap();
    let travel = EntryDoc::parse(w.decrypt(&w.repo(), "Work/travel").into_bytes()).unwrap();
    w.write().rename("Work/travel", "Shared/travel", Some(&travel)).unwrap();
}

fn queued(w: &World) -> Vec<(ChangeKind, String, Option<String>)> {
    queue::unpushed(&w.repo()).unwrap().into_iter().map(|i| (i.kind, i.entry, i.previous)).collect()
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

    let report = operation::send(&w.server.remote(), &signer(&w.card), &w.repo()).unwrap();
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
fn a_paw_edit_of_an_entry_the_desktop_also_edited_is_kept_twice() {
    let w = world();
    let mut edit = w.open("Work/github.com");
    edit.set_password("phone-pw");
    w.write().save(&edit).unwrap();
    w.desktop_push(&[("Work/github.com.gpg", encrypted("desktop-pw\n"))], "Edit on the desktop.");

    let Integration::Integrated(report) = w.pull() else { panic!("expected an integrate") };

    assert_eq!(report.kept_both.len(), 1);
    let KeptBoth { entry, copy } = &report.kept_both[0];
    assert_eq!(entry, "Work/github.com");
    assert_eq!(w.decrypt(&w.repo(), "Work/github.com"), "desktop-pw\n");
    assert_eq!(w.decrypt(&w.repo(), copy), "phone-pw\nlogin: you\n");
}

#[test]
fn a_paw_rename_of_an_entry_the_desktop_deleted_still_lands_under_its_new_name() {
    let w = world();
    w.write().rename("Work/old", "Work/older", None).unwrap();
    git(w.desktop.path(), &["rm", "--quiet", "Work/old.gpg"]);
    w.desktop_push(&[], "Remove Work/old from store.");

    let Integration::Integrated(report) = w.pull() else { panic!("expected an integrate") };

    // The rename's delete has nothing left to remove; its new name is new.
    assert_eq!(report.replayed, 1);
    assert_eq!(w.decrypt(&w.repo(), "Work/older"), "old\n");
    assert!(!w.repo().join("Work/old.gpg").exists());
}

#[test]
fn a_paw_delete_of_an_entry_the_desktop_edited_is_dropped() {
    let w = world();
    w.write().delete("Work/Jira").unwrap();
    w.desktop_push(&[("Work/Jira.gpg", encrypted("jira v2\n"))], "Edit on the desktop.");

    let Integration::Integrated(report) = w.pull() else { panic!("expected an integrate") };

    assert_eq!(report.dropped_deletes, ["Work/Jira"]);
    assert_eq!(w.decrypt(&w.repo(), "Work/Jira"), "jira v2\n");
    assert!(queued(&w).is_empty());
}

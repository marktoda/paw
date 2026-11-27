//! Re-encrypting held-back changes by hand, as the sync sheet's key task
//! does it: a pull gives a folder a new list of keys, the phone's waiting
//! changes for it are held back, and `reencrypt_with_key` opens each with
//! the (simulated) card and encrypts it for the new list, one touch each.
//! A second simulated card stands in for the new recipient, so the tests
//! can check the result really opens for them.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use std::path::Path;
use std::sync::{Arc, OnceLock, Weak};

use key_common::{Driver, PIN, decrypt, finish, signer};
use paw_core::sim::remote::steps;
use paw_core::ffi::sync::StoreSync;
use paw_core::key::{KeyError, KeyPort};
use paw_core::task::{KeyOutcome, KeyTask, KeyTaskListener, KeyTaskState, TaskProgress};
use paw_core::sim::copies::SimClock;
use paw_core::sim::{SimTouch, SimulatedKey};
use paw_core::sync::operation::SyncState;
use paw_core::sync::queue;
use paw_core::write::held_back::{self, LeftHeldBack, LeftReason, ReencryptReport};
use paw_core::sync::{self, ChangeKind, Connection};
use paw_core::write::{EntryDraft, Keyring, WriteStore};
use support::{GitServer, git, move_entry};

/// A fingerprint no key paw has is for.
const UNKNOWN: &str = "0123456789ABCDEF0123456789ABCDEF01234567";

struct World {
    card: Arc<SimulatedKey>,
    /// The new recipient's card.
    other: Arc<SimulatedKey>,
    server: GitServer,
    desktop: tempfile::TempDir,
    state: tempfile::TempDir,
    port: Arc<KeyPort>,
    sync: Arc<StoreSync>,
}

impl World {
    fn repo(&self) -> std::path::PathBuf {
        self.state.path().join(paw_core::files::layout::REPO)
    }

    fn signer(&self) -> key_common::KeySigner {
        signer(&self.card)
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

    fn pull(&self) {
        let connection = Connection::open(&self.server.remote(), &self.signer()).unwrap();
        steps::fetch(&connection, &self.repo()).unwrap();
        sync::integrate(&self.repo()).unwrap();
    }

    fn save(&self, folder: &str, name: &str, password: &str) {
        let mut draft = EntryDraft::new(folder);
        draft.set_name(name);
        draft.set_password(password);
        WriteStore::open(self.state.path()).save(&draft).unwrap();
    }

    /// The desktop gives `folder` the lines `lines`, the phone pulls and,
    /// if `accept`, accepts the new list.
    fn new_list(&self, folder: &str, lines: &[&str], accept: bool) {
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        self.desktop_push(&[(&format!("{folder}/.gpg-id"), text.into_bytes())], "New list.");
        self.pull();
        if accept {
            Keyring::open(self.state.path()).unwrap().accept_recipients(&paw_core::lock::StoreLock::for_repo(&self.repo()).exclusive().unwrap(), &self.repo(), folder).unwrap();
        }
    }

    fn held(&self) -> Vec<String> {
        self.sync.held_back().unwrap().into_iter().map(|h| h.entry).collect()
    }

    fn queued(&self) -> Vec<(ChangeKind, String)> {
        queue::unpushed(&self.repo()).unwrap().into_iter().map(|i| (i.kind, i.entry)).collect()
    }

    /// The entry's file as the queue has it now, opened with `card` (with
    /// no touch: only to check, after the task).
    fn open_with(&self, card: &Arc<SimulatedKey>, entry: &str) -> Result<String, KeyError> {
        card.set_decryption_touch(SimTouch::Off);
        let bytes = std::fs::read(self.repo().join(format!("{entry}.gpg"))).unwrap();
        decrypt(card, PIN, &bytes).map(|t| String::from_utf8(t).unwrap())
    }

    fn reencrypt(&self, driver: &Arc<Driver>) -> Arc<KeyTask> {
        let task = self.sync.clone().reencrypt_with_key(self.port.clone(), driver.clone());
        driver.drive(&task);
        task
    }
}

/// A store for the card with a `Work` folder, cloned to the phone, whose
/// keyring has the card's key and the other card's key pinned. The card
/// asks for a touch for every decryption.
fn world() -> World {
    let card = SimulatedKey::new(PIN.into(), false, 5_000);
    card.set_decryption_touch(SimTouch::EachUse);
    let other = SimulatedKey::new_other(PIN.into(), false, 5_000);
    let server = GitServer::start(&[card.ssh_public_key()]);
    let desktop = server.desktop();
    let state = tempfile::tempdir().unwrap();
    std::fs::write(state.path().join(paw_core::files::layout::REMOTE), format!("{}\n", server.sim.address())).unwrap();
    let port = KeyPort::new(SimClock::new(1_790_000_000_000));
    port.plugged_in(card.clone()).unwrap();
    let sync = StoreSync::open(state.path().to_string_lossy().into_owned()).unwrap();
    let w = World { card, other, server, desktop, state, port, sync };

    let card_fpr = w.card.fingerprint_hex();
    w.desktop_push(&[(".gpg-id", format!("{card_fpr}\n").into_bytes())], "The store.");
    let connection = Connection::open(&w.server.remote(), &w.signer()).unwrap();
    steps::clone(&connection, &w.server.remote(), &w.repo()).unwrap();
    let mut keyring = Keyring::open(w.state.path()).unwrap();
    keyring.pin_card_key(&keyring.lock().exclusive().unwrap(), &w.card.decryption_key().unwrap()).unwrap();
    keyring.pin_card_key(&keyring.lock().exclusive().unwrap(), &w.other.decryption_key().unwrap()).unwrap();
    keyring.accept_all_recipients_at_setup(&keyring.lock().exclusive().unwrap(), &w.repo()).unwrap();
    w
}

/// Three changes saved for the card alone, then `Work` gets the other card
/// too, accepted on the phone.
fn three_held_back() -> World {
    let w = world();
    w.save("Work", "a", "a-pw");
    w.save("Work", "b", "b-pw");
    w.save("Work", "c", "c-pw");
    let (card, other) = (w.card.fingerprint_hex(), w.other.fingerprint_hex());
    w.new_list("Work", &[&card, &other], true);
    assert_eq!(w.held(), ["Work/a", "Work/b", "Work/c"]);
    w
}

fn progress(done: u32, total: u32) -> impl Fn(&KeyTaskState) -> bool {
    move |s| matches!(s, KeyTaskState::Working { progress: Some(p) } if *p == TaskProgress { done, total, bytes: None })
}

#[test]
fn every_held_back_change_is_re_encrypted_for_the_new_list_with_a_touch_each_and_then_sent() {
    let w = three_held_back();
    let before = w.queued();
    assert_eq!(w.sync.reencrypt_check().unwrap().ready, 3);
    let driver = Driver::helpful(PIN, &w.card);
    assert!(!w.port.session().unwrap().holds_pin());

    let task = w.reencrypt(&driver);

    assert_eq!(reencrypted(&task), ReencryptReport { reencrypted: 3, left: vec![] });
    assert_eq!(w.card.decipher_count(), 3);
    assert_eq!(driver.count(key_common::is_touch), 3, "{:?}", driver.states());
    // Asked once, at the task's first stage, which the listener is not told about.
    assert_eq!(driver.count(key_common::is_pin), 0, "{:?}", driver.states());
    for (done, total) in [(0, 3), (1, 3), (2, 3), (3, 3)] {
        assert!(driver.saw(progress(done, total)), "{done} of {total}: {:?}", driver.states());
    }
    assert!(w.held().is_empty());
    assert_eq!(w.queued(), before, "the same changes, in the same order");
    // Each opens for the new recipient as well as for the card.
    for (entry, password) in [("Work/a", "a-pw\n"), ("Work/b", "b-pw\n"), ("Work/c", "c-pw\n")] {
        assert_eq!(w.open_with(&w.other, entry).unwrap(), password);
        assert_eq!(w.open_with(&w.card, entry).unwrap(), password);
    }

    let report = steps::sync(&w.server.remote(), &w.signer(), &w.repo(), w.state.path(), true).unwrap();
    assert_eq!(report.state, SyncState::UpToDate);
    assert_eq!(report.pushed, 3);
}

/// Cancels the task the first time it reports `done` changes finished.
struct CancelAfter {
    done: u32,
    task: OnceLock<Weak<KeyTask>>,
    inner: Arc<Driver>,
}

impl KeyTaskListener for CancelAfter {
    fn state_changed(&self, state: KeyTaskState) {
        let reached = matches!(&state, KeyTaskState::Working { progress: Some(p) } if p.done == self.done);
        self.inner.state_changed(state);
        if reached && let Some(task) = self.task.get().and_then(Weak::upgrade) {
            task.cancel();
        }
    }
}

/// What a re-encrypt that ran to the end did.
fn reencrypted(task: &Arc<KeyTask>) -> ReencryptReport {
    match finish(task) {
        KeyTaskState::Done { outcome: KeyOutcome::Reencrypted { report } } => report,
        other => panic!("expected a finished re-encrypt, got {other:?}"),
    }
}

#[test]
fn a_cancel_part_way_keeps_what_was_done_and_a_second_run_does_only_the_rest() {
    let w = three_held_back();
    let before = w.queued();
    let listener = Arc::new(CancelAfter { done: 1, task: OnceLock::new(), inner: Driver::helpful(PIN, &w.card) });
    let task = w.sync.clone().reencrypt_with_key(w.port.clone(), listener.clone());
    let _ = listener.task.set(Arc::downgrade(&task));
    listener.inner.drive(&task);

    assert!(matches!(finish(&task), KeyTaskState::Cancelled));
    task.join();
    assert_eq!(w.card.decipher_count(), 1);
    assert_eq!(w.held(), ["Work/b", "Work/c"], "the first one is done, the rest still held");
    assert_eq!(w.queued(), before, "nothing lost");
    assert_eq!(w.open_with(&w.other, "Work/a").unwrap(), "a-pw\n");
    // Nothing is sent while any is held back.
    let tip = w.server.tip();
    let report = steps::sync(&w.server.remote(), &w.signer(), &w.repo(), w.state.path(), true).unwrap();
    assert_eq!(report.state, SyncState::Waiting { unpushed: 3 });
    assert_eq!(w.server.tip(), tip);

    let driver = Driver::helpful(PIN, &w.card);
    let task = w.reencrypt(&driver);

    assert_eq!(reencrypted(&task), ReencryptReport { reencrypted: 2, left: vec![] });
    assert_eq!(w.card.decipher_count(), 3, "the first one was not opened again");
    assert!(driver.saw(progress(0, 2)) && driver.saw(progress(2, 2)), "{:?}", driver.states());
    assert!(w.held().is_empty());
    assert_eq!(w.queued(), before);
    for (entry, password) in [("Work/a", "a-pw\n"), ("Work/b", "b-pw\n"), ("Work/c", "c-pw\n")] {
        assert_eq!(w.open_with(&w.other, entry).unwrap(), password);
    }
}

/// Commits `entry` encrypted for the other card only, as paw would word
/// it: a waiting change the plugged-in card can't open.
fn commit_for_the_other_card(w: &World, entry: &str) {
    let key = w.other.decryption_key().unwrap();
    let bytes = paw_core::sim::fixture::encrypt(&[key], b"theirs\n").unwrap();
    let file = w.repo().join(format!("{entry}.gpg"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, bytes).unwrap();
    git(&w.repo(), &["add", &format!("{entry}.gpg")]);
    git(&w.repo(), &["commit", "--quiet", "-m", &format!("Add given password for {entry} to store.")]);
}

#[test]
fn changes_that_cannot_be_re_encrypted_stay_held_back_and_say_why() {
    let w = world();
    let (card, other) = (w.card.fingerprint_hex(), w.other.fingerprint_hex());
    w.save("Work", "ok", "ok-pw");
    commit_for_the_other_card(&w, "Work/theirs");
    w.save("Personal", "x", "x-pw");
    w.save("Later", "y", "y-pw");
    // Work: both cards. Personal: a key paw has never seen. Later: changed
    // but not accepted.
    w.new_list("Work", &[&card, &other], true);
    w.new_list("Personal", &[&card, UNKNOWN], true);
    w.new_list("Later", &[&card, &other], false);
    assert_eq!(w.held().len(), 4);
    let check = w.sync.reencrypt_check().unwrap();
    assert_eq!(check.ready, 2, "{check:?}");
    let driver = Driver::helpful(PIN, &w.card);

    let task = w.reencrypt(&driver);

    let report = reencrypted(&task);
    assert_eq!(report.reencrypted, 1);
    let held = w.sync.held_back().unwrap();
    let id = |entry: &str| held.iter().find(|h| h.entry == entry).unwrap().id.clone();
    let left = |entry: &str, reason: LeftReason| LeftHeldBack { id: id(entry), entry: entry.into(), reason };
    assert_eq!(
        report.left,
        [
            left("Work/theirs", LeftReason::NotForThisKey),
            left("Personal/x", LeftReason::MissingKeys { missing: vec![UNKNOWN.into()] }),
            left("Later/y", LeftReason::NotAccepted),
        ]
    );
    assert_eq!(w.card.decipher_count(), 1, "no touch for the ones that can't be done");
    assert_eq!(w.open_with(&w.other, "Work/ok").unwrap(), "ok-pw\n");
    // The ones left are exactly as they were: still only for the card.
    assert!(w.open_with(&w.other, "Personal/x").is_err());
    assert_eq!(w.open_with(&w.card, "Personal/x").unwrap(), "x-pw\n");
    let tip = w.server.tip();
    let report = steps::sync(&w.server.remote(), &w.signer(), &w.repo(), w.state.path(), true).unwrap();
    assert!(matches!(report.state, SyncState::Waiting { .. }));
    assert_eq!(w.server.tip(), tip, "nothing sent while some are held back");
}

#[test]
fn with_nothing_held_back_it_does_nothing() {
    let w = world();
    w.save("Work", "a", "a-pw");
    assert_eq!(w.sync.reencrypt_check().unwrap(), held_back::ReencryptCheck { ready: 0, left: vec![] });
    let head = git(&w.repo(), &["rev-parse", "HEAD"]);
    let driver = Driver::helpful(PIN, &w.card);

    let task = w.reencrypt(&driver);

    assert_eq!(reencrypted(&task), ReencryptReport { reencrypted: 0, left: vec![] });
    assert_eq!(w.card.decipher_count(), 0);
    assert_eq!(git(&w.repo(), &["rev-parse", "HEAD"]), head);
}

#[test]
fn a_save_while_each_one_is_opened_does_not_add_up_to_a_failure() {
    let w = world();
    for name in ["a", "b", "c", "d", "e"] {
        w.save("Work", name, &format!("{name}-pw"));
    }
    let (card, other) = (w.card.fingerprint_hex(), w.other.fingerprint_hex());
    w.new_list("Work", &[&card, &other], true);
    assert_eq!(w.held().len(), 5);
    w.card.set_decryption_touch(SimTouch::Off);
    let mut saved = 0;
    // Each open saves something else, so the queue moves under every one.
    let mut open = |_: &held_back::HeldBack, bytes: &[u8]| {
        saved += 1;
        w.save("Notes", &format!("n{saved}"), "pw");
        decrypt(&w.card, PIN, bytes)
            .map(zeroize::Zeroizing::new)
            .map_err(|e| held_back::OpenFailure::Stop(paw_core::sync::SyncError::Fault { fault: paw_core::fault::Fault::Local { detail: e.to_string() } }))
    };

    let report = held_back::reencrypt_each(&w.repo(), w.state.path(), &mut open, &mut |_, _| {}).unwrap();

    assert_eq!(report, ReencryptReport { reencrypted: 5, left: vec![] });
    assert_eq!(saved, 5);
    assert!(w.held().is_empty());
}

#[test]
fn a_rename_that_kept_its_ciphertext_is_opened_once() {
    let w = world();
    w.save("Work", "a", "a-pw");
    move_entry(&WriteStore::open(w.state.path()), "Work/a", "Work/b", None).unwrap();
    let (card, other) = (w.card.fingerprint_hex(), w.other.fingerprint_hex());
    w.new_list("Work", &[&card, &other], true);
    assert_eq!(w.held(), ["Work/a", "Work/b"]);
    let driver = Driver::helpful(PIN, &w.card);

    let task = w.reencrypt(&driver);

    assert_eq!(reencrypted(&task).reencrypted, 2);
    assert_eq!(w.card.decipher_count(), 1, "one touch for one ciphertext");
    assert!(w.held().is_empty());
    assert_eq!(w.open_with(&w.other, "Work/b").unwrap(), "a-pw\n");
    assert!(!Path::new(&w.repo().join("Work/a.gpg")).exists());
}

/// Starts a re-encrypt with the PIN not held and types it once, at the
/// task's first stage, as the sheet's field does: nothing types it again.
fn reencrypt_typing_the_pin_once(w: &World) -> (Arc<KeyTask>, Arc<Driver>) {
    let toucher = Driver::toucher(&w.card);
    let task = w.sync.clone().reencrypt_with_key(w.port.clone(), toucher.clone());
    assert!(matches!(task.state(), KeyTaskState::Pin { .. }), "{:?}", task.state());
    task.supply_pin(PIN.as_bytes().to_vec());
    (task, toucher)
}

#[test]
fn a_pin_typed_at_the_first_stage_is_the_one_used() {
    let w = three_held_back();

    let (task, driver) = reencrypt_typing_the_pin_once(&w);

    assert_eq!(reencrypted(&task), ReencryptReport { reencrypted: 3, left: vec![] });
    assert_eq!(driver.count(key_common::is_pin), 0, "not asked again: {:?}", driver.states());
    assert!(w.held().is_empty());
}

#[test]
fn a_re_encrypt_cancelled_at_the_pin_runs_in_full_when_started_again() {
    let w = three_held_back();
    let task = w.sync.clone().reencrypt_with_key(w.port.clone(), Driver::new());
    assert!(matches!(task.state(), KeyTaskState::Pin { .. }), "{:?}", task.state());
    task.cancel();
    assert!(matches!(finish(&task), KeyTaskState::Cancelled));
    task.join();
    assert_eq!(w.held(), ["Work/a", "Work/b", "Work/c"]);

    let (task, _) = reencrypt_typing_the_pin_once(&w);

    assert_eq!(reencrypted(&task), ReencryptReport { reencrypted: 3, left: vec![] });
    assert!(w.held().is_empty());
}

/// A cancel at the touch can't stop the card's wait: a re-encrypt started
/// at once waits at `Busy` until that wait ends (the card's touch timeout,
/// or a touch, whose answer is thrown away), then asks for its own touch.
#[test]
fn a_re_encrypt_cancelled_at_the_touch_and_started_at_once_waits_for_the_card_then_runs() {
    let w = three_held_back();
    let typist = Driver::typist(PIN);
    let task = w.sync.clone().reencrypt_with_key(w.port.clone(), typist.clone());
    typist.drive(&task);
    typist.wait_for(key_common::is_touch);
    let cancelled_at = std::time::Instant::now();
    task.cancel();
    assert!(matches!(finish(&task), KeyTaskState::Cancelled));

    let driver = Driver::helpful(PIN, &w.card);
    let again = w.reencrypt(&driver);

    assert!(matches!(again.state(), KeyTaskState::Busy), "{:?}", again.state());
    driver.wait_for(key_common::is_touch);
    // The simulated card gives up waiting for a touch after 5 s (`world`).
    assert!(cancelled_at.elapsed() >= std::time::Duration::from_secs(4), "{:?}", cancelled_at.elapsed());
    assert_eq!(reencrypted(&again), ReencryptReport { reencrypted: 3, left: vec![] });
    assert_eq!(driver.count(key_common::is_touch), 3, "{:?}", driver.states());
    assert_eq!(driver.count(key_common::is_pin), 0, "the PIN is still held");
    task.join();
    assert_eq!(w.card.decipher_count(), 3);
}

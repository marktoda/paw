//! First-run setup through the key session, as the setup screens drive it:
//! each step that needs the key is a key task on the `KeyPort`, the PIN is
//! typed once for the whole setup, and the git host is the simulated one
//! (`SimRemote`), which serves the fixture store with no `git` program.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use std::sync::Arc;

use key_common::{Driver, PIN, WAIT, finish, is_pin, is_touch};
use paw_core::ffi::sync::{Setup, StoreSync};
use paw_core::key::{KeyError, KeyIdentity, KeyOutcome, KeyPort, KeyTaskListener, KeyTaskState, enrolment};
use paw_core::setup::outcome::{RemoteMessage, RemoteReason};
use paw_core::setup::{SetupError, SetupState};
use paw_core::sim::copies::SimClock;
use paw_core::sim::fixture::other_key_fingerprint_hex;
use paw_core::sim::{SimTouch, SimulatedKey};
use paw_core::write::{EntryDraft, WriteError, WriteStore};
use support::GitServer;

/// The fixture store has 29 entries, 27 of them for the simulated key.
const ENTRIES: u32 = 29;

/// A key whose SSH login slot keeps a touch for a while (YubiKey "cached")
/// and whose decryption slot wants one every time.
fn cached_login_key() -> Arc<SimulatedKey> {
    key_common::sim(SimTouch::EachUse, SimTouch::Cached, WAIT)
}

struct World {
    sim: Arc<SimulatedKey>,
    port: Arc<KeyPort>,
    server: GitServer,
    dir: tempfile::TempDir,
    setup: Arc<Setup>,
}

fn world_with(sim: Arc<SimulatedKey>) -> World {
    let server = GitServer::start_with_fixture(&[sim.ssh_public_key()]);
    let port = KeyPort::new(SimClock::new(1_790_000_000_000));
    port.attach(sim.clone()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let setup = Setup::open(dir.path().to_string_lossy().into_owned()).unwrap();
    World { sim, port, server, dir, setup }
}

fn world() -> World {
    world_with(cached_login_key())
}

fn address(world: &World) -> String {
    world.server.sim.address()
}

/// Reads the key from its session, sets the address and trusts the host:
/// no PIN and no touch so far.
fn to_need_pin(w: &World) {
    let session = w.port.session().unwrap();
    assert!(matches!(w.setup.read_key_from_session(session).unwrap(), SetupState::Address { .. }));
    w.setup.set_address(address(w)).unwrap();
    let SetupState::ConfirmHost { fingerprint, .. } = w.setup.check_host().unwrap() else { panic!("expected ConfirmHost") };
    assert_eq!(fingerprint, w.server.sim.host_key_fingerprint());
    assert!(matches!(w.setup.confirm_host(fingerprint).unwrap(), SetupState::NeedPin { .. }));
    assert_eq!(w.sim.verify_attempts(), 0);
    assert_eq!(w.sim.touch_count(), 0);
}

fn download(w: &World, driver: &Arc<Driver>) -> KeyTaskState {
    let task = w.setup.clone().download_with_key(w.port.clone(), driver.clone());
    driver.drive(&task);
    finish(&task)
}

/// Runs both key steps of setup. Returns their drivers and how many PIN
/// stages the person saw in all (a task that opens at the PIN stage shows
/// it without telling its listener).
fn full_setup(w: &World) -> (Arc<Driver>, Arc<Driver>, usize) {
    to_need_pin(w);
    let downloading = Driver::helpful(PIN, &w.sim);
    let task = w.setup.clone().download_with_key(w.port.clone(), downloading.clone());
    let mut pins = usize::from(is_pin(&task.state()));
    downloading.drive(&task);
    let state = finish(&task);
    assert!(matches!(state, KeyTaskState::Done { outcome: KeyOutcome::SignedIn }), "{state:?}");
    assert!(matches!(w.setup.state(), SetupState::TestDecrypt { entries: ENTRIES, .. }));

    let checking = Driver::helpful(PIN, &w.sim);
    let task = w.setup.clone().test_decrypt_with_key(w.port.clone(), checking.clone());
    pins += usize::from(is_pin(&task.state()));
    checking.drive(&task);
    let state = finish(&task);
    assert!(matches!(state, KeyTaskState::Done { outcome: KeyOutcome::KeyChecked }), "{state:?}");
    pins += downloading.count(is_pin) + checking.count(is_pin);
    (downloading, checking, pins)
}

fn writer(w: &World) -> WriteStore {
    WriteStore::open(w.dir.path())
}

fn new_entry(name: &str) -> EntryDraft {
    let mut draft = EntryDraft::new("");
    draft.set_name(name);
    draft.set_password("a new password");
    draft
}

#[test]
fn a_whole_setup_asks_for_the_pin_once_and_touches_as_the_policies_say() {
    let w = world();
    let (downloading, checking, pins) = full_setup(&w);

    let SetupState::Done { opened, entries } = w.setup.state() else { panic!("{:?}", w.setup.state()) };
    assert_eq!(entries, ENTRIES);
    assert!(!opened.starts_with("archive/"), "{opened}");

    // One PIN stage in the whole setup, and one VERIFY with a PIN in it.
    assert_eq!(pins, 1);
    assert_eq!(w.sim.verify_attempts(), 1);
    // Reading the key, the address and the host check needed no touch. The
    // login signed once (one touch, which the cached policy then keeps for
    // a while) and the test decrypt took one: two touches in all.
    assert_eq!(w.sim.authentication_count(), 1);
    assert_eq!(w.sim.decipher_count(), 1);
    assert_eq!(w.sim.touch_count(), 2);
    assert_eq!(w.server.logins(), 1);
    assert!(downloading.saw(is_touch) && checking.saw(is_touch));
    // Progress was reported while cloning.
    assert!(downloading.saw(|s| matches!(s, KeyTaskState::Working { progress: Some(_) })));
    assert_eq!(w.setup.last_task_result().unwrap(), w.setup.state());

    // A refresh right after, inside the cached window: no PIN, no touch.
    let sync = StoreSync::open(w.dir.path().to_string_lossy().into_owned()).unwrap();
    let refreshing = Driver::helpful(PIN, &w.sim);
    let task = sync.clone().refresh_with_key(w.port.clone(), refreshing.clone());
    refreshing.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { .. }));
    assert!(sync.last_task_result().unwrap().is_some());
    assert!(!refreshing.saw(is_pin) && !refreshing.saw(is_touch));
    assert_eq!(w.sim.verify_attempts(), 1);
    assert_eq!(w.sim.authentication_count(), 2);
    assert_eq!(w.sim.touch_count(), 2);
}

#[test]
fn after_setup_a_new_entry_saves_with_no_further_step() {
    let w = world();
    full_setup(&w);

    let ready = w.setup.write_readiness().unwrap();
    assert!(ready.can_save, "{ready:?}");
    assert!(ready.missing_recipients.is_empty());
    // The fixture's `archive` folder is for a key paw does not have.
    assert_eq!(ready.folders_needing_keys, ["archive/.gpg-id"]);

    writer(&w).save(&new_entry("new-site")).unwrap();
    assert!(w.dir.path().join("repo/new-site.gpg").is_file());

    // And it goes out with the next send, on the held PIN.
    let sync = StoreSync::open(w.dir.path().to_string_lossy().into_owned()).unwrap();
    let sending = Driver::helpful(PIN, &w.sim);
    let task = sync.clone().send_with_key(w.port.clone(), sending.clone());
    sending.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { .. }), "{:?}", sync.last_task_result());
    let report = sync.last_task_result().unwrap().unwrap();
    assert_eq!(report.pushed, 1);
    assert_eq!(w.server.sim.tip(), Some(support::git(&w.dir.path().join("repo"), &["rev-parse", "HEAD"])));
    assert_eq!(w.sim.verify_attempts(), 1);
}

#[test]
fn recipients_paw_has_no_key_for_leave_setup_done_but_say_saving_is_not_possible_yet() {
    let w = world();
    let other = other_key_fingerprint_hex().unwrap();
    let card = w.sim.fingerprint_hex();
    w.server.desktop_commit(".gpg-id", &format!("{card}\n{other}\n"));
    full_setup(&w);

    // Reading works: setup opened an entry and is done.
    assert!(matches!(w.setup.state(), SetupState::Done { .. }));
    let ready = w.setup.write_readiness().unwrap();
    assert!(!ready.can_save);
    assert_eq!(ready.missing_recipients, [other.as_str()]);
    // The other recipient's line was not bound to the card.
    assert!(matches!(
        writer(&w).save(&new_entry("new-site")).unwrap_err(),
        WriteError::MissingRecipients { missing, .. } if missing == [other]
    ));
}

#[test]
fn setup_records_the_key_and_gives_it_to_the_port() {
    let w = world();
    full_setup(&w);
    let recorded = enrolment::load_enrolment(w.dir.path()).unwrap().unwrap();
    assert_eq!(recorded.decryption_fingerprint.as_deref(), Some(w.sim.fingerprint_hex().as_str()));
    assert_eq!(recorded.authentication_fingerprint.as_deref(), Some(w.sim.authentication_fingerprint_hex().as_str()));
    assert_eq!(recorded.serial(), "12345678");
    assert_eq!(w.port.enrolled_key(), Some(recorded));
    assert!(w.port.key_matches_enrolment());
}

#[test]
fn the_old_one_shot_setup_completes_the_write_path_too() {
    // The per-call steps (connection and PIN each time) end the same way.
    let w = world_with(key_common::sim(SimTouch::Off, SimTouch::Off, WAIT));
    let mut flow = paw_core::setup::SetupFlow::open(w.dir.path()).unwrap();
    flow.read_key(w.sim.clone()).unwrap();
    flow.set_address(&address(&w)).unwrap();
    let SetupState::ConfirmHost { fingerprint, .. } = flow.check_host().unwrap() else { panic!() };
    flow.confirm_host(&fingerprint).unwrap();
    struct Quiet;
    impl paw_core::setup::SetupEvents for Quiet {
        fn touch_needed(&self, _: paw_core::setup::touch::TouchExpectation) {}
        fn progress(&self, _: u32, _: u32) -> bool {
            true
        }
    }
    flow.download(w.sim.clone(), PIN, Arc::new(Quiet)).unwrap();
    flow.test_decrypt(w.sim.clone(), PIN, Arc::new(Quiet)).unwrap();
    assert!(flow.write_readiness().unwrap().can_save);
    assert!(enrolment::load_enrolment(w.dir.path()).unwrap().is_some());
    writer(&w).save(&new_entry("new-site")).unwrap();
}

#[test]
fn a_cancelled_download_goes_back_to_the_pin_step_and_starts_over_without_a_new_pin() {
    let w = world();
    to_need_pin(&w);

    // Cancel at the first progress report.
    struct CancelOnProgress {
        inner: Arc<Driver>,
        task: std::sync::OnceLock<std::sync::Weak<paw_core::key::KeyTask>>,
    }
    impl KeyTaskListener for CancelOnProgress {
        fn state_changed(&self, state: KeyTaskState) {
            let progressed = matches!(state, KeyTaskState::Working { progress: Some(_) });
            self.inner.state_changed(state);
            if progressed && let Some(task) = self.task.get().and_then(std::sync::Weak::upgrade) {
                task.cancel();
            }
        }
    }
    let inner = Driver::helpful(PIN, &w.sim);
    let listener = Arc::new(CancelOnProgress { inner: inner.clone(), task: std::sync::OnceLock::new() });
    let task = w.setup.clone().download_with_key(w.port.clone(), listener.clone());
    let _ = listener.task.set(Arc::downgrade(&task));
    inner.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Cancelled));
    task.join();

    assert!(matches!(w.setup.state(), SetupState::NeedPin { .. }));
    assert!(!w.dir.path().join("repo").exists());
    let again = Driver::helpful(PIN, &w.sim);
    assert!(matches!(download(&w, &again), KeyTaskState::Done { .. }));
    assert!(!again.saw(is_pin));
    assert_eq!(w.sim.verify_attempts(), 1);
}

// --- the simulated host's failures -----------------------------------------

#[test]
fn a_refused_key_is_reported_at_the_host_check_before_any_pin() {
    let w = world();
    w.server.sim.set_refuse_key(true);
    let session = w.port.session().unwrap();
    w.setup.read_key_from_session(session).unwrap();
    w.setup.set_address(address(&w)).unwrap();
    let SetupState::ConfirmHost { fingerprint, .. } = w.setup.check_host().unwrap() else { panic!() };
    let error = w.setup.confirm_host(fingerprint).unwrap_err();
    assert!(
        matches!(&error, SetupError::KeyNotAccepted { public_key, .. } if *public_key == w.sim.ssh_public_key()),
        "{error:?}"
    );
    assert_eq!(w.sim.verify_attempts(), 0);
    assert_eq!(w.sim.authentication_count(), 0);

    // Added to the account: it goes on.
    w.server.sim.set_refuse_key(false);
    assert!(matches!(w.setup.check_host().unwrap(), SetupState::NeedPin { .. }));
}

#[test]
fn a_changed_host_key_stops_setup_with_both_fingerprints() {
    let w = world();
    to_need_pin(&w);
    let pinned = w.server.sim.host_key_fingerprint();
    w.server.sim.set_changed_host_key(true);
    let presented = w.server.sim.host_key_fingerprint();
    assert_ne!(pinned, presented);

    // Checked again before the PIN: refused, nothing sent.
    let error = w.setup.check_host().unwrap_err();
    assert!(
        matches!(&error, SetupError::HostKeyChanged { expected, presented: p, .. } if *expected == [pinned.clone()] && *p == presented),
        "{error:?}"
    );

    // At the download, after the PIN: refused before the key signs.
    let driver = Driver::helpful(PIN, &w.sim);
    let state = download(&w, &driver);
    assert!(matches!(state, KeyTaskState::Failed { error: KeyError::Reader { .. } }), "{state:?}");
    assert!(matches!(w.setup.last_task_result(), Err(SetupError::HostKeyChanged { .. })));
    assert_eq!(w.sim.authentication_count(), 0);
    assert_eq!(w.server.logins(), 0);
    assert!(matches!(w.setup.state(), SetupState::NeedPin { .. }));
}

#[test]
fn a_connection_dropped_mid_clone_fails_cleanly_and_the_next_try_works() {
    let w = world();
    to_need_pin(&w);
    w.server.sim.set_drop_mid_clone(true);
    let driver = Driver::helpful(PIN, &w.sim);
    let state = download(&w, &driver);
    assert!(matches!(state, KeyTaskState::Failed { error: KeyError::Reader { .. } }), "{state:?}");
    assert!(w.setup.last_task_result().is_err());
    assert!(matches!(w.setup.state(), SetupState::NeedPin { .. }));
    assert!(!w.dir.path().join("repo").exists());

    w.server.sim.set_drop_mid_clone(false);
    let again = Driver::helpful(PIN, &w.sim);
    assert!(matches!(download(&w, &again), KeyTaskState::Done { .. }));
    assert!(w.setup.last_task_result().is_ok());
    assert!(!again.saw(is_pin));
    assert_eq!(w.sim.verify_attempts(), 1);
    assert!(matches!(w.setup.state(), SetupState::TestDecrypt { entries: ENTRIES, .. }));
}

#[test]
fn another_key_plugged_in_mid_setup_is_refused_before_the_pin() {
    let w = world();
    to_need_pin(&w);
    let other = SimulatedKey::other_with_options(paw_core::sim::SimOptions {
        pin: PIN.into(),
        decrypt_touch: false,
        auth_touch: false,
        touch_timeout: WAIT,
        extended_length: false,
    });
    w.port.attach(other.clone()).unwrap();
    let driver = Driver::helpful(PIN, &other);
    let task = w.setup.clone().download_with_key(w.port.clone(), driver.clone());
    assert!(matches!(finish(&task), KeyTaskState::Failed { error: KeyError::NotForThisKey { .. } }));
    let mismatch = task.key_mismatch().unwrap();
    assert_eq!(mismatch.presented, KeyIdentity {
        ident: "0006:87654321".into(),
        decryption_fingerprint: Some(other.fingerprint_hex()),
        authentication_fingerprint: Some(other.authentication_fingerprint_hex()),
    });
    assert_eq!(other.verify_attempts() + other.pin_queries(), 0);
}

#[test]
fn a_repository_the_key_cannot_see_is_told_apart_from_a_refused_key() {
    let w = world();
    // A key the host does not know: found at the host check.
    w.server.sim.set_refuse_key(true);
    let session = w.port.session().unwrap();
    w.setup.read_key_from_session(session).unwrap();
    w.setup.set_address(address(&w)).unwrap();
    let SetupState::ConfirmHost { fingerprint, .. } = w.setup.check_host().unwrap() else { panic!() };
    assert!(matches!(w.setup.confirm_host(fingerprint).unwrap_err(), SetupError::KeyNotAccepted { .. }));
    assert_eq!(w.setup.last_remote_message().unwrap().reason, RemoteReason::KeyNotAccepted);
    w.server.sim.set_refuse_key(false);
    assert!(matches!(w.setup.check_host().unwrap(), SetupState::NeedPin { .. }));
    assert_eq!(w.setup.last_remote_message(), None);

    // The key logs in, and the host says there is no such repository for it.
    w.server.sim.set_repository_not_found(true);
    let driver = Driver::helpful(PIN, &w.sim);
    let state = download(&w, &driver);

    assert!(matches!(state, KeyTaskState::Failed { error: KeyError::Reader { .. } }), "{state:?}");
    let Err(SetupError::Failed { message }) = w.setup.last_task_result() else { panic!() };
    assert!(message.contains("doesn't exist, or this key can't see it"), "{message}");
    assert_eq!(
        w.setup.last_remote_message(),
        Some(RemoteMessage {
            reason: RemoteReason::RepositoryNotFound,
            host: "127.0.0.1".into(),
            repository: "store.git".into(),
            said: Some("ERROR: Repository not found.".into()),
        })
    );
    assert_eq!(w.server.logins(), 1);
    assert!(matches!(w.setup.state(), SetupState::NeedPin { .. }));
    assert!(!w.dir.path().join("repo").exists());

    // Access granted: the next try goes through and the message is gone.
    w.server.sim.set_repository_not_found(false);
    assert!(matches!(download(&w, &Driver::helpful(PIN, &w.sim)), KeyTaskState::Done { .. }));
    assert_eq!(w.setup.last_remote_message(), None);
}

#[test]
fn an_empty_repository_sends_the_task_path_back_to_the_address_with_the_reason() {
    let sim = cached_login_key();
    let server = GitServer::start_empty(&[sim.ssh_public_key()]);
    let port = KeyPort::new(SimClock::new(1_790_000_000_000));
    port.attach(sim.clone()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let setup = Setup::open(dir.path().to_string_lossy().into_owned()).unwrap();
    let w = World { sim, port, server, dir, setup };
    to_need_pin(&w);

    let state = download(&w, &Driver::helpful(PIN, &w.sim));

    assert!(matches!(state, KeyTaskState::Failed { error: KeyError::Reader { .. } }), "{state:?}");
    assert!(matches!(w.setup.last_task_result(), Err(SetupError::EmptyRepository)));
    assert!(matches!(w.setup.state(), SetupState::Address { .. }), "{:?}", w.setup.state());
    assert_eq!(w.setup.last_remote_message().unwrap().reason, RemoteReason::EmptyRepository);
    assert!(!w.dir.path().join("repo").exists());
    let relaunched = Setup::open(w.dir.path().to_string_lossy().into_owned()).unwrap();
    assert_eq!(relaunched.last_remote_message().unwrap().reason, RemoteReason::EmptyRepository);
}

#[test]
fn an_expected_host_key_change_is_forgotten_and_confirmed_again_without_leaving_setup() {
    let w = world();
    to_need_pin(&w);
    let old = w.server.sim.host_key_fingerprint();
    w.server.sim.set_changed_host_key(true);
    let new = w.server.sim.host_key_fingerprint();
    let Err(SetupError::HostKeyChanged { expected, presented, .. }) = w.setup.check_host() else { panic!() };
    assert_eq!((expected, presented.clone()), (vec![old], new.clone()));

    assert!(matches!(w.setup.forget_host_key().unwrap(), SetupState::CheckHost { .. }));

    let SetupState::ConfirmHost { fingerprint, .. } = w.setup.check_host().unwrap() else { panic!() };
    assert_eq!(fingerprint, new);
    assert!(matches!(w.setup.confirm_host(fingerprint).unwrap(), SetupState::NeedPin { .. }));
    assert!(matches!(download(&w, &Driver::helpful(PIN, &w.sim)), KeyTaskState::Done { .. }));
}

#[test]
fn a_bundled_hosts_key_cannot_be_forgotten() {
    for address in ["git@github.com:you/store.git", "ssh://git@ssh.github.com:443/you/store.git", "git@gitlab.com:you/store.git"] {
        let w = world();
        let session = w.port.session().unwrap();
        w.setup.read_key_from_session(session).unwrap();
        w.setup.set_address(address.into()).unwrap();
        let known = paw_core::setup::known_hosts(w.dir.path());
        let host = if address.contains("ssh.github") { "ssh.github.com" } else { address.split(['@', ':']).nth(1).unwrap() };
        let port = if address.contains(":443") { 443 } else { 22 };
        // Even a key someone managed to save for it beside the bundled ones.
        known.add(host, port, &support::some_other_host_key()).unwrap();

        let error = w.setup.forget_host_key().unwrap_err();

        assert!(matches!(&error, SetupError::Failed { message } if message.contains("can't be forgotten")), "{error:?}");
        assert!(known.lookup(host, port).unwrap().is_some());
        // The bundled keys are what counts for it, whatever was saved.
        let pinned = paw_core::sync::hosts::pin_for(&known, host, port).unwrap().unwrap();
        assert_eq!(pinned.fingerprints(), paw_core::sync::hosts::bundled(host, port).unwrap().fingerprints());
    }
}

#[test]
fn settings_cannot_forget_a_bundled_hosts_key_either() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("remote"), "git@github.com:you/store.git\n").unwrap();
    let sync = StoreSync::open(dir.path().to_string_lossy().into_owned()).unwrap();

    let error = sync.forget_host_key().unwrap_err();

    assert!(matches!(&error, paw_core::sync::SyncError::Failed { message } if message.contains("can't be forgotten")), "{error:?}");
}

#[test]
fn a_pin_kdf_paw_does_not_support_stops_setup_at_your_key_before_any_pin() {
    for (algorithm, enabled, supported) in [(None, false, true), (Some(3), true, true), (Some(7), true, false)] {
        let sim = cached_login_key();
        if let Some(algorithm) = algorithm {
            sim.set_pin_kdf(algorithm);
        }
        let port = KeyPort::new(SimClock::new(1_790_000_000_000));
        let session = port.attach(sim.clone()).unwrap();
        let status = session.pin_kdf();
        assert_eq!((status.enabled, status.supported), (enabled, supported), "{algorithm:?}");
        assert_eq!(session.pin_kdf_supported(), supported);
        assert_eq!(status.detail.is_some(), !supported);

        let dir = tempfile::tempdir().unwrap();
        let setup = Setup::open(dir.path().to_string_lossy().into_owned()).unwrap();
        let read = setup.read_key_from_session(session);
        if supported {
            assert!(matches!(read, Ok(SetupState::Address { .. })), "{read:?}");
        } else {
            assert!(matches!(&read, Err(SetupError::UnusableKey { message }) if message.contains("KDF")), "{read:?}");
            assert_eq!(setup.state(), SetupState::ReadKey);
        }

        // The older path, straight from the card, says the same.
        let flow_dir = tempfile::tempdir().unwrap();
        let mut flow = paw_core::setup::SetupFlow::open(flow_dir.path()).unwrap();
        assert_eq!(flow.read_key(sim.clone()).is_ok(), supported, "{algorithm:?}");
        assert_eq!(sim.verify_attempts() + sim.pin_queries(), 0);
    }
}

#[test]
fn a_download_reports_objects_and_bytes_received() {
    let w = world();
    to_need_pin(&w);
    assert_eq!(w.setup.transfer_progress(), None);
    let driver = Driver::helpful(PIN, &w.sim);
    let task = w.setup.clone().download_with_key(w.port.clone(), driver.clone());
    driver.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { .. }));

    let transfer = task.transfer_progress().unwrap();
    assert!(transfer.total_objects > 0);
    assert_eq!(transfer.received_objects, transfer.total_objects);
    assert!(transfer.received_bytes > 0);
    assert_eq!(w.setup.transfer_progress(), Some(transfer));
    // The sheet saw it as Working with objects done and expected, ending
    // at the total.
    assert!(driver.saw(|state| matches!(
        state,
        KeyTaskState::Working { progress: Some(p) } if p.done == transfer.total_objects && p.total == transfer.total_objects
    )));
}

#[test]
fn a_refresh_reports_what_its_fetch_received() {
    let w = world();
    full_setup(&w);
    w.server.sim.commit_file("new.gpg".into(), vec![7; 4096], "From the desktop.".into()).unwrap();
    let sync = StoreSync::open(w.dir.path().to_string_lossy().into_owned()).unwrap();
    let driver = Driver::helpful(PIN, &w.sim);
    let task = sync.clone().refresh_with_key(w.port.clone(), driver.clone());
    driver.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { .. }), "{:?}", sync.last_task_result());

    let transfer = task.transfer_progress().unwrap();
    assert!(transfer.received_objects > 0 && transfer.received_objects == transfer.total_objects, "{transfer:?}");
    assert!(transfer.received_bytes > 0);
}

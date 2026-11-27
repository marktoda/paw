//! First-run setup through the key session, as the setup screens drive it:
//! each step that needs the key is a key task on the `KeyPort`, the PIN is
//! typed once for the whole setup, and the git host is the simulated one
//! (`SimRemote`), which serves the fixture store with no `git` program.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use std::sync::Arc;

use key_common::{Driver, PIN, WAIT, finish, is_pin, is_touch, port_with};
use paw_core::setup::Setup;
use paw_core::task::StoreSync;
use paw_core::key::{KeyError, KeyIdentity, KeyPort, enrolment};
use paw_core::task::{KeyOutcome, KeyTaskListener, KeyTaskState};
use paw_core::remote::RemoteFailure;
use paw_core::setup::{SetupError, SetupState, UnusableKeyReason};
use paw_core::sim::copies::SimClock;
use paw_core::sim::fixture::other_key_fingerprint_hex;
use paw_core::sim::{SimTouch, SimulatedKey};
use paw_core::write::{Draft, WriteError, WriteStore};
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

fn world_on(sim: Arc<SimulatedKey>, server: GitServer) -> World {
    let port = port_with(&sim);
    let dir = tempfile::tempdir().unwrap();
    let setup = Setup::open(dir.path().to_string_lossy().into_owned()).unwrap();
    World { sim, port, server, dir, setup }
}

fn world_with(sim: Arc<SimulatedKey>) -> World {
    let server = GitServer::start_with_fixture(&[sim.ssh_public_key()]);
    world_on(sim, server)
}

fn world() -> World {
    world_with(cached_login_key())
}

/// Some other key, with the same PIN.
fn other_key() -> Arc<SimulatedKey> {
    SimulatedKey::other_with_options(paw_core::sim::SimOptions {
        pin: PIN.into(),
        decrypt_touch: false,
        auth_touch: false,
        touch_timeout: WAIT,
        extended_length: false,
    })
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
/// stages the person saw in all.
fn full_setup(w: &World) -> (Arc<Driver>, Arc<Driver>, usize) {
    to_need_pin(w);
    let downloading = Driver::helpful(PIN, &w.sim);
    let task = w.setup.clone().download_with_key(w.port.clone(), downloading.clone());
    downloading.drive(&task);
    let state = finish(&task);
    assert!(matches!(state, KeyTaskState::Done { outcome: KeyOutcome::SignedIn }), "{state:?}");
    assert!(matches!(w.setup.state(), SetupState::TestDecrypt { entries: ENTRIES, .. }));

    let checking = Driver::helpful(PIN, &w.sim);
    let task = w.setup.clone().test_decrypt_with_key(w.port.clone(), checking.clone());
    checking.drive(&task);
    let state = finish(&task);
    assert!(matches!(state, KeyTaskState::Done { outcome: KeyOutcome::KeyChecked }), "{state:?}");
    let pins = downloading.count(is_pin) + checking.count(is_pin);
    (downloading, checking, pins)
}

fn writer(w: &World) -> WriteStore {
    WriteStore::open(w.dir.path())
}

fn new_entry(name: &str) -> Draft {
    let mut draft = Draft::new("");
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

    // A refresh right after, inside the cached window: no PIN, no touch.
    let sync = StoreSync::open(w.dir.path().to_string_lossy().into_owned()).unwrap();
    let refreshing = Driver::helpful(PIN, &w.sim);
    let task = sync.clone().refresh_with_key(w.port.clone(), refreshing.clone());
    refreshing.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { outcome: KeyOutcome::Synced { .. } }));
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

    writer(&w).save(&new_entry("new-site")).unwrap();
    assert!(w.dir.path().join("repo/new-site.gpg").is_file());

    // And it goes out with the next send, on the held PIN.
    let sync = StoreSync::open(w.dir.path().to_string_lossy().into_owned()).unwrap();
    let sending = Driver::helpful(PIN, &w.sim);
    let task = sync.clone().send_with_key(w.port.clone(), sending.clone());
    sending.drive(&task);
    let state = finish(&task);
    let KeyTaskState::Done { outcome: KeyOutcome::Synced { report } } = state else { panic!("{state:?}") };
    assert_eq!(report.pushed, 1);
    assert_eq!(w.server.sim.tip(), Some(support::git(&w.dir.path().join(paw_core::files::layout::REPO), &["rev-parse", "HEAD"])));
    assert_eq!(w.sim.verify_attempts(), 1);
}

#[test]
fn a_send_over_a_change_paw_did_not_make_still_finds_the_remote_replaced() {
    let w = world();
    full_setup(&w);
    let repo = w.dir.path().join(paw_core::files::layout::REPO);
    // Another tool commits two entries at once: not a change paw makes.
    std::fs::write(repo.join("one.gpg"), "1").unwrap();
    std::fs::write(repo.join("two.gpg"), "2").unwrap();
    support::git(&repo, &["add", "one.gpg", "two.gpg"]);
    support::git(&repo, &["commit", "--quiet", "-m", "Two at once, by another tool."]);
    assert!(paw_core::sync::queue::unpushed(&paw_core::lock::StoreDirs::new(w.dir.path())).is_err());
    // The desktop starts the remote over.
    let desktop = w.server.desktop();
    support::git(desktop.path(), &["checkout", "--quiet", "--orphan", "fresh"]);
    support::git(desktop.path(), &["rm", "-r", "--quiet", "--cached", "."]);
    std::fs::write(desktop.path().join(".gpg-id"), "AAAA1111\n").unwrap();
    support::git(desktop.path(), &["add", ".gpg-id"]);
    support::git(desktop.path(), &["commit", "--quiet", "-m", "Start over."]);
    support::git(desktop.path(), &["push", "--quiet", "--force", "origin", "HEAD:refs/heads/main"]);

    let sync = StoreSync::open(w.dir.path().to_string_lossy().into_owned()).unwrap();
    let sending = Driver::helpful(PIN, &w.sim);
    let task = sync.clone().send_with_key(w.port.clone(), sending.clone());
    sending.drive(&task);
    let state = finish(&task);

    let KeyTaskState::Done { outcome: KeyOutcome::Synced { report } } = state else { panic!("{state:?}") };
    assert!(matches!(report.state, paw_core::sync::operation::SyncState::RemoteReplaced { .. }), "{report:?}");
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
    let recorded = enrolment::load_enrolment(&paw_core::lock::StoreDirs::new(w.dir.path())).unwrap().unwrap();
    assert_eq!(recorded.decryption_fingerprint.as_deref(), Some(w.sim.fingerprint_hex().as_str()));
    assert_eq!(recorded.authentication_fingerprint.as_deref(), Some(w.sim.authentication_fingerprint_hex().as_str()));
    assert_eq!(recorded.serial(), "12345678");
    assert!(w.port.key_matches_enrolment());
    // The port now holds out for that key: another one is not it.
    w.port.plugged_in(other_key()).unwrap();
    assert_eq!(w.port.key_mismatch().unwrap().enrolled, recorded);
}

#[test]
fn a_cancelled_download_goes_back_to_the_pin_step_and_starts_over_without_a_new_pin() {
    let w = world();
    to_need_pin(&w);

    // Cancel at the first progress report.
    struct CancelOnProgress {
        inner: Arc<Driver>,
        task: std::sync::OnceLock<std::sync::Weak<paw_core::task::KeyTask>>,
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
    assert!(!w.dir.path().join(paw_core::files::layout::REPO).exists());
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
        matches!(&error, SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::KeyNotAccepted { public_key, .. } } } if *public_key == w.sim.ssh_public_key()),
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
        matches!(&error, SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::HostKeyChanged { expected, presented: p, bundled: false, .. } } } if *expected == [pinned.clone()] && *p == presented),
        "{error:?}"
    );

    // At the download, after the PIN: refused before the key signs.
    let driver = Driver::helpful(PIN, &w.sim);
    let state = download(&w, &driver);
    assert!(
        matches!(state, KeyTaskState::SetupFailed { error: SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::HostKeyChanged { .. } } } }),
        "{state:?}"
    );
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
    assert!(matches!(state, KeyTaskState::SetupFailed { .. }), "{state:?}");
    assert!(matches!(w.setup.state(), SetupState::NeedPin { .. }));
    assert!(!w.dir.path().join(paw_core::files::layout::REPO).exists());

    w.server.sim.set_drop_mid_clone(false);
    let again = Driver::helpful(PIN, &w.sim);
    assert!(matches!(download(&w, &again), KeyTaskState::Done { .. }));
    assert!(!again.saw(is_pin));
    assert_eq!(w.sim.verify_attempts(), 1);
    assert!(matches!(w.setup.state(), SetupState::TestDecrypt { entries: ENTRIES, .. }));
}

#[test]
fn another_key_plugged_in_mid_setup_is_refused_before_the_pin() {
    let w = world();
    to_need_pin(&w);
    let other = other_key();
    w.port.plugged_in(other.clone()).unwrap();
    let driver = Driver::helpful(PIN, &other);
    let task = w.setup.clone().download_with_key(w.port.clone(), driver.clone());
    let state = finish(&task);
    let KeyTaskState::Failed { error: KeyError::DifferentKey { mismatch } } = state else { panic!("{state:?}") };
    assert_eq!(mismatch.presented, KeyIdentity {
        ident: "0006:87654321".into(),
        decryption_fingerprint: Some(other.fingerprint_hex()),
        authentication_fingerprint: Some(other.authentication_fingerprint_hex()),
    });
    assert_eq!(other.verify_attempts() + other.pin_queries(), 0);
}

#[test]
fn another_key_plugged_in_before_the_test_decrypt_is_refused_before_the_pin() {
    let w = world();
    to_need_pin(&w);
    assert!(matches!(download(&w, &Driver::helpful(PIN, &w.sim)), KeyTaskState::Done { .. }));
    let other = other_key();
    w.port.plugged_in(other.clone()).unwrap();

    let driver = Driver::helpful(PIN, &other);
    let task = w.setup.clone().test_decrypt_with_key(w.port.clone(), driver.clone());
    driver.drive(&task);

    let state = finish(&task);
    let KeyTaskState::Failed { error: KeyError::DifferentKey { mismatch } } = state else { panic!("{state:?}") };
    assert_eq!(mismatch.presented.ident, "0006:87654321");
    assert_eq!(other.verify_attempts() + other.pin_queries(), 0);
    assert_eq!(other.decipher_count(), 0);
    assert!(matches!(w.setup.state(), SetupState::TestDecrypt { .. }));
    assert!(enrolment::load_enrolment(&paw_core::lock::StoreDirs::new(w.dir.path())).unwrap().is_none());
}

#[test]
fn a_repository_the_key_cannot_see_is_told_apart_from_a_refused_key() {
    let w = world();
    to_need_pin(&w);

    // The key logs in, and the host says there is no such repository for it.
    w.server.sim.set_repository_not_found(true);
    let driver = Driver::helpful(PIN, &w.sim);
    let state = download(&w, &driver);

    let KeyTaskState::SetupFailed { error: SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure } } } = state else { panic!("{state:?}") };
    let RemoteFailure::RepositoryMissing { host, repository, said, .. } = failure else { panic!("{failure:?}") };
    assert_eq!(
        (host.as_str(), repository.as_str(), said.as_deref()),
        ("127.0.0.1", "store.git", Some("ERROR: Repository not found."))
    );
    assert_eq!(w.server.logins(), 1);
    assert!(matches!(w.setup.state(), SetupState::NeedPin { .. }));
    assert!(!w.dir.path().join(paw_core::files::layout::REPO).exists());

    // Access granted: the next try goes through.
    w.server.sim.set_repository_not_found(false);
    assert!(matches!(download(&w, &Driver::helpful(PIN, &w.sim)), KeyTaskState::Done { .. }));
}

#[test]
fn an_empty_repository_sends_the_task_path_back_to_the_address_with_the_reason() {
    let sim = cached_login_key();
    let server = GitServer::start_empty(&[sim.ssh_public_key()]);
    let w = world_on(sim, server);
    to_need_pin(&w);

    let state = download(&w, &Driver::helpful(PIN, &w.sim));

    assert!(matches!(state, KeyTaskState::SetupFailed { error: SetupError::EmptyRepository }), "{state:?}");
    assert!(matches!(w.setup.state(), SetupState::Address { .. }), "{:?}", w.setup.state());
    assert!(!w.dir.path().join(paw_core::files::layout::REPO).exists());
    let relaunched = Setup::open(w.dir.path().to_string_lossy().into_owned()).unwrap();
    assert!(matches!(relaunched.state(), SetupState::Address { .. }));

    // The same address again, once the store is pushed: it goes through.
    let desktop = w.server.desktop();
    paw_core::sim::fixture::write_fixture_store(desktop.path().to_string_lossy().into_owned()).unwrap();
    support::git(desktop.path(), &["add", "-A"]);
    support::git(desktop.path(), &["commit", "--quiet", "-m", "The store."]);
    support::git(desktop.path(), &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    relaunched.set_address(address(&w)).unwrap();
    assert!(matches!(relaunched.check_host().unwrap(), SetupState::NeedPin { .. }));
    let driver = Driver::helpful(PIN, &w.sim);
    let task = relaunched.clone().download_with_key(w.port.clone(), driver.clone());
    driver.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { .. }));
    assert!(matches!(relaunched.state(), SetupState::TestDecrypt { .. }));
    assert!(!std::fs::read_to_string(w.dir.path().join(paw_core::files::layout::SETUP)).unwrap().contains("problem="));
}

#[test]
fn an_expected_host_key_change_is_forgotten_and_confirmed_again_without_leaving_setup() {
    let w = world();
    to_need_pin(&w);
    let old = w.server.sim.host_key_fingerprint();
    w.server.sim.set_changed_host_key(true);
    let new = w.server.sim.host_key_fingerprint();
    let Err(SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::HostKeyChanged { expected, presented, .. } } }) = w.setup.check_host()
    else {
        panic!()
    };
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
        let store = paw_core::lock::StoreDirs::new(w.dir.path());
        let known = paw_core::sync::hosts::KnownHosts::of(&store);
        let host = if address.contains("ssh.github") { "ssh.github.com" } else { address.split(['@', ':']).nth(1).unwrap() };
        let port = if address.contains(":443") { 443 } else { 22 };
        // Even a key someone managed to save for it beside the bundled ones
        // (paw itself refuses to).
        assert!(known.add(&store.exclusive().unwrap(), host, port, &support::some_other_host_key()).is_err());
        let line = format!("{host} {port} {}\n", support::some_other_host_key());
        std::fs::write(w.dir.path().join(paw_core::files::layout::KNOWN_HOSTS), line).unwrap();

        let error = w.setup.forget_host_key().unwrap_err();

        assert!(matches!(&error, SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::HostKeyBundled { .. } } }), "{error:?}");
        assert!(known.lookup(host, port).unwrap().is_some());
        // The bundled keys are what counts for it, whatever was saved.
        let pinned = known.pin(host, port).unwrap().unwrap();
        assert_eq!(pinned.fingerprints(), paw_core::sync::hosts::bundled(host, port).unwrap().fingerprints());
    }
}

#[test]
fn a_pin_kdf_paw_does_not_support_stops_setup_at_your_key_before_any_pin() {
    for (algorithm, enabled, supported) in [(None, false, true), (Some(3), true, true), (Some(7), true, false)] {
        let sim = cached_login_key();
        if let Some(algorithm) = algorithm {
            sim.set_pin_kdf(algorithm);
        }
        let port = KeyPort::new(SimClock::new(1_790_000_000_000));
        let session = port.plugged_in(sim.clone()).unwrap();
        let status = session.pin_kdf();
        assert_eq!((status.enabled, status.supported), (enabled, supported), "{algorithm:?}");
        assert_eq!(status.detail.is_some(), !supported);

        let dir = tempfile::tempdir().unwrap();
        let setup = Setup::open(dir.path().to_string_lossy().into_owned()).unwrap();
        let read = setup.read_key_from_session(session);
        if supported {
            assert!(matches!(read, Ok(SetupState::Address { .. })), "{read:?}");
        } else {
            assert!(matches!(&read, Err(SetupError::UnusableKey { reason: UnusableKeyReason::PinKdfUnsupported { .. } })), "{read:?}");
            assert_eq!(setup.state(), SetupState::ReadKey);
        }
        assert_eq!(sim.verify_attempts() + sim.pin_queries(), 0);
    }
}

/// The simulated key, answering as if some of its slots had no key
/// generated: the card reports an all-zero fingerprint for each of `blank`
/// (0 signing, 1 decryption, 2 authentication).
struct BlankSlots {
    sim: Arc<SimulatedKey>,
    blank: Vec<usize>,
}

impl paw_core::key::CardConnection for BlankSlots {
    fn transmit(&self, apdu: Vec<u8>) -> Result<Vec<u8>, paw_core::key::ConnectionError> {
        let mut response = self.sim.transmit(apdu.clone())?;
        let fingerprints = match apdu.get(1..4) {
            // GET DATA of the fingerprints alone.
            Some([0xCA, 0x00, 0xC5]) => Some(0),
            // Within the application related data, as tag C5, 60 bytes.
            Some([0xCA, 0x00, 0x6E]) => response.windows(2).position(|w| w == [0xC5, 0x3C]).map(|at| at + 2),
            _ => None,
        };
        if let Some(start) = fingerprints {
            for slot in &self.blank {
                response[start + slot * 20..start + (slot + 1) * 20].fill(0);
            }
        }
        Ok(response)
    }
}

#[test]
fn a_key_with_no_keys_generated_is_refused_before_any_pin() {
    for (blank, reason) in
        [(vec![1, 2], UnusableKeyReason::NoDecryptionKey), (vec![1], UnusableKeyReason::NoDecryptionKey), (vec![2], UnusableKeyReason::NoAuthenticationKey)]
    {
        let sim = cached_login_key();
        let port = KeyPort::new(SimClock::new(1_790_000_000_000));
        let session = port.plugged_in(Arc::new(BlankSlots { sim: sim.clone(), blank: blank.clone() })).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let setup = Setup::open(dir.path().to_string_lossy().into_owned()).unwrap();

        let read = setup.read_key_from_session(session);

        assert!(matches!(&read, Err(SetupError::UnusableKey { reason: found }) if *found == reason), "{blank:?}: {read:?}");
        assert_eq!(setup.state(), SetupState::ReadKey);
        assert_eq!(sim.verify_attempts() + sim.pin_queries(), 0);
    }
}

#[test]
fn a_download_reports_objects_and_bytes_received() {
    let w = world();
    to_need_pin(&w);
    let driver = Driver::helpful(PIN, &w.sim);
    let task = w.setup.clone().download_with_key(w.port.clone(), driver.clone());
    driver.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { .. }));

    // The sheet saw it as Working with objects and bytes received, ending
    // at the total.
    let last = driver
        .states()
        .into_iter()
        .rev()
        .find_map(|state| match state {
            KeyTaskState::Working { progress: Some(p) } => Some(p),
            _ => None,
        })
        .unwrap();
    assert!(last.total > 0);
    assert_eq!(last.done, last.total);
    assert!(last.bytes.is_some_and(|bytes| bytes > 0));
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
    assert!(matches!(finish(&task), KeyTaskState::Done { .. }));

    // The sheet saw the objects come in, ending at the total.
    assert!(driver.saw(|state| matches!(
        state,
        KeyTaskState::Working { progress: Some(p) } if p.total > 0 && p.done == p.total
    )));
}

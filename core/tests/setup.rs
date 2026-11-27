//! First-run setup as a state machine, against the in-process git host:
//! every step, every failure, resuming after the app is killed, the address
//! forms, and how host keys are trusted. The steps that need the key run as
//! key tasks on a port with the simulated key plugged in.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use key_common::{Driver, PIN, WAIT, finish, is_touch, port_with};
use paw_core::ffi::sync::Setup;
use paw_core::key::{KeyError, KeyPort, KeyTouch};
use paw_core::task::KeyTaskState;
use paw_core::sync::address::{AddressProblem, StoreAddress};
use paw_core::remote::RemoteFailure;
use paw_core::key::touch_clock::{TouchClock, TouchExpectation};
use paw_core::setup::{SetupError, SetupFlow, SetupState};
use paw_core::sim::{SimTouch, SimulatedKey};
use paw_core::ssh::SshPublicKey;
use paw_core::sync::hosts::{self, KnownHosts};
use paw_core::remote::RefusedReason;
use paw_core::sync::{Connection, HostKeyPin};
use paw_core::write::LineStatus;
use support::{GitServer, git};

/// A key whose slots both ask for a touch as `touch` says.
fn key(touch: SimTouch) -> Arc<SimulatedKey> {
    key_common::sim(touch, touch, WAIT)
}

/// Pushes the fixture store (most of it for the simulated key) to `server`.
fn push_fixture_store(server: &GitServer) {
    let desktop = server.desktop();
    paw_core::sim::fixture::write_fixture_store(desktop.path().to_string_lossy().into_owned()).unwrap();
    git(desktop.path(), &["add", "-A"]);
    git(desktop.path(), &["commit", "--quiet", "-m", "The store."]);
    git(desktop.path(), &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
}

/// Pushes a store whose entries are all for a key the card does not hold.
fn push_foreign_store(server: &GitServer) {
    let fixture = tempfile::tempdir().unwrap();
    paw_core::sim::fixture::write_fixture_store(fixture.path().to_string_lossy().into_owned()).unwrap();
    let desktop = server.desktop();
    let archive = fixture.path().join("archive");
    std::fs::copy(archive.join(".gpg-id"), desktop.path().join(".gpg-id")).unwrap();
    for name in ["old-forum.gpg", "university.gpg"] {
        std::fs::copy(archive.join(name), desktop.path().join(name)).unwrap();
    }
    git(desktop.path(), &["add", "-A"]);
    git(desktop.path(), &["commit", "--quiet", "-m", "Someone else's store."]);
    git(desktop.path(), &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
}

fn address(server: &GitServer) -> String {
    format!("ssh://git@127.0.0.1:{}/store.git", server.remote().port)
}

fn open_setup(dir: &Path) -> Arc<Setup> {
    Setup::open(dir.to_string_lossy().into_owned()).unwrap()
}

/// What the app sees after being killed and started again.
fn relaunch(dir: &Path) -> SetupState {
    open_setup(dir).state()
}

/// A setup in a state directory of its own, for the store at `address`,
/// with `sim` plugged into `port`.
struct Rig {
    sim: Arc<SimulatedKey>,
    port: Arc<KeyPort>,
    dir: tempfile::TempDir,
    setup: Arc<Setup>,
    address: String,
}

impl Rig {
    fn new(sim: &Arc<SimulatedKey>, server: &GitServer) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let setup = open_setup(dir.path());
        Self { sim: sim.clone(), port: port_with(sim), dir, setup, address: address(server) }
    }

    /// As after a relaunch: the setup read back from its state directory.
    fn reopen(&mut self) {
        self.setup = open_setup(self.dir.path());
    }

    fn repo(&self) -> std::path::PathBuf {
        self.dir.path().join(paw_core::files::layout::REPO)
    }

    /// At `CheckHost`: the key read from its session, the address set.
    fn at_check_host(&self) {
        self.setup.read_key_from_session(self.port.session().unwrap()).unwrap();
        self.setup.set_address(self.address.clone()).unwrap();
    }

    /// At `NeedPin`: the host's key confirmed and the key accepted.
    fn at_need_pin(&self) {
        self.at_check_host();
        let SetupState::ConfirmHost { fingerprint, .. } = self.setup.check_host().unwrap() else {
            panic!("expected ConfirmHost");
        };
        assert!(matches!(self.setup.confirm_host(fingerprint).unwrap(), SetupState::NeedPin { .. }));
    }

    /// "Download your store" as a key task, driven by `driver`.
    fn download_by(&self, driver: &Arc<Driver>) -> KeyTaskState {
        let task = self.setup.clone().download_with_key(self.port.clone(), driver.clone());
        driver.drive(&task);
        finish(&task)
    }

    /// "Check your key" as a key task, driven by `driver`.
    fn test_decrypt_by(&self, driver: &Arc<Driver>) -> KeyTaskState {
        let task = self.setup.clone().test_decrypt_with_key(self.port.clone(), driver.clone());
        driver.drive(&task);
        finish(&task)
    }

    /// The download, typing the PIN and touching when asked: the state it
    /// left setup in, or the setup error it failed with.
    fn download(&self) -> Result<SetupState, SetupError> {
        let state = self.download_by(&Driver::helpful(PIN, &self.sim));
        self.result(state)
    }

    /// The test decrypt, typing the PIN and touching when asked.
    fn test_decrypt(&self) -> Result<SetupState, SetupError> {
        let state = self.test_decrypt_by(&Driver::helpful(PIN, &self.sim));
        self.result(state)
    }

    /// Done, or failed for a reason that is the setup's, not the key's.
    fn result(&self, state: KeyTaskState) -> Result<SetupState, SetupError> {
        match state {
            KeyTaskState::Done { .. } => Ok(self.setup.state()),
            KeyTaskState::SetupFailed { error } => Err(error),
            other => panic!("the task ended {other:?}"),
        }
    }
}

// --- the whole flow --------------------------------------------------------

#[test]
fn setup_reads_the_key_checks_the_host_downloads_and_opens_one_entry() {
    let sim = key(SimTouch::EachUse);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    push_fixture_store(&server);
    let rig = Rig::new(&sim, &server);
    assert_eq!(rig.setup.state(), SetupState::ReadKey);

    let SetupState::Address { key } = rig.setup.read_key_from_session(rig.port.session().unwrap()).unwrap() else {
        panic!()
    };
    assert_eq!(key.ssh_public_key, sim.ssh_public_key());
    assert_eq!(key.authentication_touch, KeyTouch::EachUse);

    let state = rig.setup.set_address(address(&server)).unwrap();
    assert_eq!(state, SetupState::CheckHost { address: address(&server), notice: None });

    // An unknown host: its key is shown before anything else happens.
    let SetupState::ConfirmHost { host, algorithm, fingerprint } = rig.setup.check_host().unwrap() else { panic!() };
    assert_eq!((host.as_str(), algorithm.as_str()), ("127.0.0.1", "ssh-ed25519"));
    assert_eq!(fingerprint, server.remote().host_key.fingerprint());

    let state = rig.setup.confirm_host(fingerprint).unwrap();
    assert_eq!(state, SetupState::NeedPin { address: address(&server), touch: TouchExpectation::Needed });
    // Checking the host and the key cost no signature and no PIN try.
    assert_eq!(sim.authentication_count(), 0);
    assert_eq!(server.logins(), 0);

    let downloading = Driver::helpful(PIN, &sim);
    let state = rig.result(rig.download_by(&downloading)).unwrap();
    assert_eq!(state, SetupState::TestDecrypt { entries: 29, touch: TouchExpectation::Needed });
    assert_eq!(sim.authentication_count(), 1);
    assert!(downloading.saw(|s| matches!(s, KeyTaskState::Working { progress: Some(_) })));
    assert_eq!(git(&rig.repo(), &["rev-parse", "HEAD"]), server.tip());

    let checking = Driver::helpful(PIN, &sim);
    let state = rig.result(rig.test_decrypt_by(&checking)).unwrap();
    let SetupState::Done { opened, entries } = state else { panic!("{state:?}") };
    assert_eq!(entries, 29);
    assert!(!opened.starts_with("archive/"), "{opened}");
    assert_eq!(sim.decipher_count(), 1);
    assert_eq!(downloading.count(is_touch) + checking.count(is_touch), 2);
    assert_eq!(sim.pin_tries_left(), 3);

    // Nothing secret is saved.
    let saved = std::fs::read_to_string(rig.dir.path().join(paw_core::files::layout::SETUP)).unwrap();
    assert!(!saved.contains(PIN), "{saved}");
    assert!(paw_core::files::layout::is_set_up(rig.dir.path()));
    // Sync can find the store.
    assert_eq!(paw_core::sync::address::saved_address(rig.dir.path()).unwrap().display(), address(&server));
}

#[test]
fn a_confirmed_host_is_pinned_and_not_asked_about_again() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    let mut rig = Rig::new(&sim, &server);
    rig.at_need_pin();

    let pinned = KnownHosts::of(rig.dir.path())
        .lookup("127.0.0.1", server.remote().port)
        .unwrap()
        .unwrap();
    assert_eq!(pinned.fingerprint(), server.remote().host_key.fingerprint());

    // Back to CheckHost: now the host is known, so it goes straight on.
    rig.reopen();
    rig.setup.back().unwrap();
    assert!(matches!(rig.setup.check_host().unwrap(), SetupState::NeedPin { .. }));
}

#[test]
fn a_wrong_fingerprint_does_not_pin_the_host() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    let rig = Rig::new(&sim, &server);
    rig.at_check_host();
    rig.setup.check_host().unwrap();

    let error = rig.setup.confirm_host("SHA256:not-what-was-shown".into()).unwrap_err();

    assert!(matches!(error, SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::HostKeyNotConfirmed { .. } } }), "{error:?}");
    assert!(!rig.dir.path().join(paw_core::files::layout::KNOWN_HOSTS).exists());
    assert!(matches!(rig.setup.state(), SetupState::ConfirmHost { .. }));
}

// --- each failure ----------------------------------------------------------

#[test]
fn an_unreachable_host_is_its_own_failure() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    let rig = Rig::new(&sim, &server);
    rig.at_check_host();
    drop(server);

    let error = rig.setup.check_host().unwrap_err();

    // Refused, or reset by the closing server.
    let host = match &error {
        SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::NoConnection { host, fallback: None, .. } } }
        | SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::Unreachable { host, .. } } } => host,
        _ => panic!("{error:?}"),
    };
    assert_eq!(host, "127.0.0.1");
    assert!(matches!(rig.setup.state(), SetupState::CheckHost { .. }));
}

#[test]
fn the_key_check_asks_without_a_signature() {
    let sim = key(SimTouch::EachUse);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    let key = SshPublicKey::ed25519(&sim.authentication_public_key());

    Connection::probe(&server.remote(), &key).unwrap();
    let other = SshPublicKey::ed25519(&[3u8; 32]);
    let refused = Connection::probe(&server.remote(), &other).unwrap_err();
    let not_accepted = RemoteFailure::KeyNotAccepted { host: "127.0.0.1".into(), public_key: other.authorized_key() };
    assert!(matches!(&refused, paw_core::fault::Fault::Remote { failure } if *failure == not_accepted), "{refused:?}");
    assert_eq!(sim.authentication_count(), 0);
    assert_eq!(server.logins(), 0);
}

#[test]
fn a_wrong_pin_reports_the_tries_left_then_the_block() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    push_fixture_store(&server);
    let rig = Rig::new(&sim, &server);
    rig.at_need_pin();
    let driver = Driver::helpful("000000", &sim);

    let state = rig.download_by(&driver);

    assert!(matches!(state, KeyTaskState::Failed { error: KeyError::PinBlocked }), "{state:?}");
    let wrong: Vec<u8> = driver
        .states()
        .into_iter()
        .filter_map(|state| match state {
            KeyTaskState::Pin { last_error: Some(KeyError::WrongPin { tries_left }), .. } => Some(tries_left),
            _ => None,
        })
        .collect();
    assert_eq!(wrong, [2, 1]);
    assert!(matches!(rig.setup.state(), SetupState::NeedPin { .. }));
    assert_eq!(server.logins(), 0);
    assert!(!rig.repo().exists());
}

#[test]
fn a_key_nobody_touches_times_out() {
    let sim = key_common::sim(SimTouch::EachUse, SimTouch::EachUse, Duration::from_millis(1500));
    let server = GitServer::start(&[sim.ssh_public_key()]);
    push_fixture_store(&server);
    let rig = Rig::new(&sim, &server);
    rig.at_need_pin();

    let state = rig.download_by(&Driver::typist(PIN));

    assert!(matches!(state, KeyTaskState::Failed { error: KeyError::TouchTimeout }), "{state:?}");
    assert!(matches!(rig.setup.state(), SetupState::NeedPin { .. }));
    assert!(!rig.repo().exists());
}

#[test]
fn a_store_with_a_link_in_it_is_refused_and_says_where() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    push_fixture_store(&server);
    server.desktop_symlink("up", Path::new(".."));
    let rig = Rig::new(&sim, &server);
    rig.at_need_pin();

    let error = rig.download().unwrap_err();

    let SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::RefusedContent { paths: refused } } } = &error else { panic!("{error:?}") };
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!((refused[0].path.as_str(), &refused[0].reasons[..]), ("up", &[RefusedReason::SymbolicLink][..]));
    assert!(!rig.repo().exists());
    assert!(matches!(rig.setup.state(), SetupState::NeedPin { .. }));
    // Once the remote is fixed, the download goes through.
    let desktop = server.desktop();
    git(desktop.path(), &["rm", "--quiet", "up"]);
    git(desktop.path(), &["commit", "--quiet", "-m", "No link."]);
    git(desktop.path(), &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    rig.download().unwrap();
}

#[test]
fn a_repository_without_gpg_id_is_not_a_pass_store() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    let rig = Rig::new(&sim, &server);
    rig.at_need_pin();

    let error = rig.download().unwrap_err();

    assert!(matches!(error, SetupError::NotAPassStore), "{error:?}");
    assert!(!rig.repo().exists());
    assert!(matches!(rig.setup.state(), SetupState::Address { .. }), "{:?}", rig.setup.state());
    assert!(matches!(open_setup(rig.dir.path()).state(), SetupState::Address { .. }));
}

#[test]
fn a_store_the_key_cannot_open_is_reported_before_any_touch() {
    let sim = key(SimTouch::EachUse);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    push_foreign_store(&server);
    let rig = Rig::new(&sim, &server);
    rig.at_need_pin();
    rig.download().unwrap();
    let touches = sim.touch_count();

    let error = rig.test_decrypt().unwrap_err();

    assert!(matches!(error, SetupError::KeyCannotOpenStore), "{error:?}");
    assert_eq!(sim.decipher_count(), 0);
    assert_eq!(sim.touch_count(), touches);
    assert!(matches!(rig.setup.state(), SetupState::TestDecrypt { .. }));
}

#[test]
fn a_step_out_of_order_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut flow = SetupFlow::open(dir.path()).unwrap();

    let error = flow.set_address("git@github.com:you/store.git").unwrap_err();

    assert!(matches!(error, SetupError::WrongStep { .. }), "{error:?}");
}

// --- resuming after a kill --------------------------------------------------

#[test]
fn setup_resumes_after_a_kill_at_every_step() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    push_fixture_store(&server);
    let mut rig = Rig::new(&sim, &server);
    let dir = rig.dir.path().to_path_buf();
    assert_eq!(relaunch(&dir), SetupState::ReadKey);

    let state = rig.setup.read_key_from_session(rig.port.session().unwrap()).unwrap();
    assert_eq!(relaunch(&dir), state);

    let state = rig.setup.set_address(address(&server)).unwrap();
    assert_eq!(relaunch(&dir), state);

    // Killed while the host's key waits for confirmation: the host is asked
    // again, so the key shown is always the one presented now.
    let confirm = rig.setup.check_host().unwrap();
    assert!(matches!(relaunch(&dir), SetupState::CheckHost { .. }));
    let SetupState::ConfirmHost { fingerprint, .. } = confirm else { panic!() };
    let state = rig.setup.confirm_host(fingerprint).unwrap();
    assert_eq!(relaunch(&dir), state);

    // Killed in the middle of the download (the state file and the
    // half-made copy as a kill leaves them): the copy is removed on
    // relaunch.
    let file = dir.join(paw_core::files::layout::SETUP);
    let saved = std::fs::read_to_string(&file).unwrap();
    let cloning: String = saved
        .lines()
        .map(|line| if line.starts_with("step=") { "step=cloning\n".to_owned() } else { format!("{line}\n") })
        .collect();
    std::fs::write(&file, cloning).unwrap();
    std::fs::create_dir_all(rig.repo().join(".git")).unwrap();
    std::fs::write(rig.repo().join("half.gpg"), b"half").unwrap();
    rig.reopen();
    assert!(matches!(rig.setup.state(), SetupState::NeedPin { .. }));
    assert!(!rig.repo().exists());

    let state = rig.download().unwrap();
    assert_eq!(relaunch(&dir), state);
    let logins = server.logins();

    // Killed before the test decrypt: no second download.
    rig.reopen();
    let state = rig.test_decrypt().unwrap();
    assert!(matches!(state, SetupState::Done { .. }));
    assert_eq!(relaunch(&dir), state);
    assert_eq!(server.logins(), logins);
}

#[test]
fn a_store_that_went_missing_after_the_download_is_downloaded_again() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    push_fixture_store(&server);
    let rig = Rig::new(&sim, &server);
    rig.at_need_pin();
    rig.download().unwrap();
    std::fs::remove_dir_all(rig.repo()).unwrap();

    assert!(matches!(relaunch(rig.dir.path()), SetupState::NeedPin { .. }));
}

#[test]
fn an_unreadable_state_file_starts_setup_over() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(paw_core::files::layout::SETUP), format!("{}\n", paw_core::files::layout::SETUP_DONE)).unwrap();

    assert_eq!(relaunch(dir.path()), SetupState::ReadKey);
}

// --- touch -----------------------------------------------------------------

#[test]
fn a_cached_touch_is_not_asked_for_again_within_its_window() {
    let now = Instant::now();
    let mut cached = TouchClock::new(KeyTouch::Cached);
    assert_eq!(cached.expectation_at(now), TouchExpectation::Needed);
    cached.used_at(now);
    assert_eq!(cached.expectation_at(now + Duration::from_secs(5)), TouchExpectation::NotNeeded);
    assert_eq!(cached.expectation_at(now + Duration::from_secs(14)), TouchExpectation::MayBeNeeded);
    assert_eq!(cached.expectation_at(now + Duration::from_secs(60)), TouchExpectation::Needed);

    let mut each = TouchClock::new(KeyTouch::EachUse);
    each.used_at(now);
    assert_eq!(each.expectation_at(now), TouchExpectation::Needed);
    assert_eq!(TouchClock::new(KeyTouch::Off).expectation_at(now), TouchExpectation::NotNeeded);
    assert_eq!(TouchClock::new(KeyTouch::Unknown).expectation_at(now), TouchExpectation::MayBeNeeded);
}

#[test]
fn a_second_login_within_the_cached_window_does_not_ask_for_a_touch() {
    let sim = key_common::sim(SimTouch::Off, SimTouch::Cached, WAIT);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    server.sim.set_repository_not_found(true);
    let rig = Rig::new(&sim, &server);
    rig.at_need_pin();
    assert_eq!(rig.port.session().unwrap().info().authentication_touch, KeyTouch::Cached);

    // The first login asks; the repository is not there, so it is tried again.
    let first = Driver::helpful(PIN, &sim);
    let error = rig.result(rig.download_by(&first)).unwrap_err();
    assert!(matches!(error, SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::RepositoryMissing { .. } } }), "{error:?}");
    assert_eq!(first.count(is_touch), 1);
    assert_eq!(rig.setup.state(), SetupState::NeedPin { address: address(&server), touch: TouchExpectation::NotNeeded });

    let second = Driver::helpful(PIN, &sim);
    rig.result(rig.download_by(&second)).unwrap_err();
    assert!(!second.saw(is_touch));
    assert_eq!(sim.authentication_count(), 2);
}

#[test]
fn a_key_that_needs_a_touch_each_time_is_asked_each_time() {
    let sim = key(SimTouch::EachUse);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    server.sim.set_repository_not_found(true);
    let rig = Rig::new(&sim, &server);
    rig.at_need_pin();

    for _ in 0..2 {
        let driver = Driver::helpful(PIN, &sim);
        rig.result(rig.download_by(&driver)).unwrap_err();
        assert_eq!(driver.count(is_touch), 1);
    }
    assert_eq!(sim.touch_count(), 2);
}

// --- addresses -------------------------------------------------------------

#[test]
fn addresses_are_read_in_the_forms_git_hosts_show() {
    let ok = |text: &str, user: &str, host: &str, port: u16, path: &str| {
        let parsed = StoreAddress::parse(text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
        let expected = StoreAddress { user: user.into(), host: host.into(), port, path: path.into() };
        assert_eq!(parsed.address, expected, "{text}");
        assert_eq!(parsed.notice, None, "{text}");
        // What is saved reads back the same.
        assert_eq!(StoreAddress::parse(&parsed.address.display()).unwrap().address, expected, "{text}");
    };
    ok("git@github.com:you/password-store.git", "git", "github.com", 22, "you/password-store.git");
    ok("  git@github.com:you/store  ", "git", "github.com", 22, "you/store");
    ok("git@gitlab.com:group/sub/store.git", "git", "gitlab.com", 22, "group/sub/store.git");
    ok("ssh://git@github.com/you/store.git", "git", "github.com", 22, "/you/store.git");
    ok("ssh://git@git.example.org:2222/srv/store.git", "git", "git.example.org", 2222, "/srv/store.git");
    ok("SSH://me@host/~/store.git", "me", "host", 22, "store.git");
    ok("ssh://git@[::1]:2222/store.git", "git", "[::1]", 2222, "/store.git");
}

#[test]
fn addresses_paw_cannot_use_are_refused_with_what_to_do() {
    for (text, says) in [
        ("", AddressProblem::Empty),
        ("git@github.com:you/my store.git", AddressProblem::HasSpaces),
        ("https://gitlab.com/you/store", AddressProblem::UseSsh),
        ("https://github.com/you/store/tree/main", AddressProblem::NotARepositoryPage),
        ("git://github.com/you/store.git", AddressProblem::UseSsh),
        ("file:///home/you/store", AddressProblem::UseSsh),
        ("github.com:you/store.git", AddressProblem::NoUser),
        ("/home/you/store", AddressProblem::NotGit),
        ("C:/Users/you/store", AddressProblem::NoUser),
        ("git@github.com:", AddressProblem::NoRepository),
        ("ssh://github.com/you/store.git", AddressProblem::SshWithoutUser),
        ("ssh://git@github.com:99999/you/store.git", AddressProblem::BadPort),
        ("ssh://git@github.com", AddressProblem::SshWithoutPath),
        ("git@-oProxyCommand=x:store", AddressProblem::NotGit),
        ("git@github.com:-u", AddressProblem::NoRepository),
        ("home/you:store", AddressProblem::LooksLikeFolder),
    ] {
        let problem = StoreAddress::parse(text).err().unwrap_or_else(|| panic!("{text:?} was accepted"));
        assert_eq!(problem, says, "{text:?}");
    }
}

// --- host keys --------------------------------------------------------------

#[test]
fn githubs_bundled_keys_match_their_published_fingerprints() {
    // As GitHub publishes them ("GitHub's SSH key fingerprints"), written
    // out here so that a changed key in the source cannot pass unseen.
    let published = [
        "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU",
        "SHA256:p2QAMXNIC1TJYWeIOttrVc98/R1BUFWu3/LiyKgUfQM",
        "SHA256:uNiVztksCsDhcc0u9e8BujQXVUpKZIDTMczCvj3tD2s",
    ];
    let github = hosts::bundled("github.com", 22).unwrap();
    assert_eq!(github.fingerprints(), published);
    assert!(hosts::bundled("github.com", 2222).is_none());
    assert!(hosts::bundled("codeberg.org", 22).is_none());
}

#[test]
fn every_bundled_key_parses_and_has_the_fingerprint_written_beside_it() {
    for (host, port, keys) in hosts::BUNDLED {
        assert!(!keys.is_empty(), "{host}");
        for (line, fingerprint) in keys {
            let pin = HostKeyPin::parse(line).unwrap_or_else(|e| panic!("{host}: {line}: {e}"));
            assert_eq!(pin.fingerprint(), *fingerprint, "{host}:{port}");
        }
        let pin = hosts::bundled(host, port).unwrap();
        assert_eq!(pin.fingerprints(), keys.iter().map(|(_, fp)| fp.to_string()).collect::<Vec<_>>());
        // Each host offers ed25519 first and has a key this build can use.
        assert_eq!(pin.algorithms()[0].to_string(), "ssh-ed25519", "{host}");
        assert!(hosts::is_bundled(&host.to_uppercase(), port));
    }
    // GitHub's port-443 endpoint has GitHub's keys, and only on 443.
    assert_eq!(
        hosts::bundled("ssh.github.com", 443).unwrap().fingerprints(),
        hosts::bundled("github.com", 22).unwrap().fingerprints()
    );
    assert!(hosts::bundled("ssh.github.com", 22).is_none());
    assert!(hosts::bundled("github.com", 443).is_none());
}

#[test]
fn only_the_pinned_key_types_are_offered() {
    let github = hosts::bundled("github.com", 22).unwrap();
    let offered: Vec<String> = github.algorithms().iter().map(ToString::to_string).collect();
    // RSA is pinned but not offered: this build has no RSA.
    assert_eq!(offered, ["ssh-ed25519", "ecdsa-sha2-nistp256"]);

    let one = HostKeyPin::parse(&support::ecdsa_host_key()).unwrap();
    let offered: Vec<String> = one.algorithms().iter().map(ToString::to_string).collect();
    assert_eq!(offered, ["ecdsa-sha2-nistp256"]);
}

#[test]
fn a_host_with_several_keys_presents_the_pinned_one() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start_with_two_host_keys(&[sim.ssh_public_key()]);
    let signer = key_common::signer(&sim);

    // Pinned to its second key: offering only that type makes the host
    // present it, where by default it would present ed25519.
    for line in [support::ecdsa_host_key(), support::ed25519_host_key()] {
        let mut remote = server.remote();
        remote.host_key = HostKeyPin::parse(&line).unwrap();
        Connection::open(&remote, &signer).unwrap_or_else(|e| panic!("{line}: {e}"));
    }
    assert_eq!(server.logins(), 2);
}

#[test]
fn a_known_hosts_entry_is_only_replaced_by_forgetting_it() {
    let dir = tempfile::tempdir().unwrap();
    let known = KnownHosts::of(dir.path());
    known.add("git.example.org", 22, &support::ed25519_host_key()).unwrap();

    assert!(known.add("git.example.org", 22, &support::ecdsa_host_key()).is_err());
    known.forget("git.example.org", 22).unwrap();
    known.add("git.example.org", 22, &support::ecdsa_host_key()).unwrap();
    let pin = known.lookup("git.example.org", 22).unwrap().unwrap();
    assert_eq!(pin.lines(), [HostKeyPin::parse(&support::ecdsa_host_key()).unwrap().lines()[0].clone()]);
    assert!(known.lookup("git.example.org", 2222).unwrap().is_none());
}

// --- which .gpg-id line setup binds to the card -----------------------------

/// Pushes `files`, each `(path, contents)`, from the desktop.
fn push_files(server: &GitServer, files: &[(&str, Vec<u8>)]) {
    let desktop = server.desktop();
    for (path, contents) in files {
        let file = desktop.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, contents).unwrap();
    }
    git(desktop.path(), &["add", "-A"]);
    git(desktop.path(), &["commit", "--quiet", "-m", "The store."]);
    git(desktop.path(), &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
}

fn encrypted_for(keys: &[pgp::packet::PublicKey], text: &str) -> Vec<u8> {
    paw_core::sim::fixture::encrypt(keys, text.as_bytes()).unwrap()
}

/// Sets up `server`'s store with `sim` and returns the state directory.
fn set_up(sim: &Arc<SimulatedKey>, server: &GitServer) -> tempfile::TempDir {
    let rig = Rig::new(sim, server);
    rig.at_need_pin();
    rig.download().unwrap();
    let done = rig.test_decrypt().unwrap();
    assert!(matches!(done, SetupState::Done { .. }), "{done:?}");
    rig.dir
}

#[test]
fn a_team_line_at_the_top_is_not_bound_to_the_card_that_opened_a_personal_entry() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    let mine = sim.decryption_key().unwrap();
    let team = paw_core::sim::fixture::other_key().unwrap();
    // The top names a team key paw has not got; `personal/` names the owner
    // by a line that does not name the card's subkey by itself.
    push_files(
        &server,
        &[
            (".gpg-id", b"team@example.com\n".to_vec()),
            ("shared.gpg", encrypted_for(&[team], "team secret\n")),
            ("personal/.gpg-id", b"owner@example.com\n".to_vec()),
            ("personal/bank.gpg", encrypted_for(&[mine], "mine\n")),
        ],
    );

    let state = set_up(&sim, &server);

    let keyring = paw_core::write::Keyring::open(&paw_core::files::layout::StoreDirs::new(state.path())).unwrap();
    assert_eq!(keyring.line_status("team@example.com"), LineStatus::Missing);
    assert!(matches!(keyring.line_status("owner@example.com"), LineStatus::Ready { bound: true, .. }));
    let readiness = open_setup(state.path()).write_readiness().unwrap();
    assert!(!readiness.can_save);
    assert_eq!(readiness.missing_recipients, ["team@example.com"]);
    assert!(readiness.folders_needing_keys.is_empty(), "{readiness:?}");
}

#[test]
fn the_one_line_of_the_gpg_id_that_governs_the_opened_entry_is_bound() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    let mine = sim.decryption_key().unwrap();
    push_files(
        &server,
        &[(".gpg-id", b"owner@example.com\n".to_vec()), ("bank.gpg", encrypted_for(&[mine], "mine\n"))],
    );

    let state = set_up(&sim, &server);

    let keyring = paw_core::write::Keyring::open(&paw_core::files::layout::StoreDirs::new(state.path())).unwrap();
    assert!(matches!(keyring.line_status("owner@example.com"), LineStatus::Ready { bound: true, .. }));
    let readiness = open_setup(state.path()).write_readiness().unwrap();
    assert!(readiness.can_save, "{readiness:?}");
}

#[test]
fn nothing_is_bound_when_the_opened_entry_does_not_name_the_card() {
    let sim = key(SimTouch::Off);
    let server = GitServer::start(&[sim.ssh_public_key()]);
    let mine = sim.decryption_key().unwrap();
    let hidden = {
        // A hidden recipient: the packet does not say which key it is for.
        let mut bytes = encrypted_for(&[mine], "mine\n");
        // A v3 PKESK: tag, length, version 3, then the eight key-ID bytes.
        let start = if bytes[0] & 0x40 != 0 { 2 } else { 2 + (bytes[0] & 3) as usize };
        let start = if bytes[0] & 0x40 != 0 && bytes[1] >= 192 { 3 } else { start };
        assert_eq!(bytes[start], 3, "expected a v3 recipient packet");
        bytes[start + 1..start + 9].fill(0);
        bytes
    };
    push_files(&server, &[(".gpg-id", b"owner@example.com\n".to_vec()), ("bank.gpg", hidden)]);

    let state = set_up(&sim, &server);

    let keyring = paw_core::write::Keyring::open(&paw_core::files::layout::StoreDirs::new(state.path())).unwrap();
    assert_eq!(keyring.line_status("owner@example.com"), LineStatus::Missing);
    let readiness = open_setup(state.path()).write_readiness().unwrap();
    assert_eq!(readiness.missing_recipients, ["owner@example.com"]);
}

//! First-run setup as a state machine, against the in-process git host:
//! every step, every failure, resuming after the app is killed, the address
//! forms, and how host keys are trusted.

#![cfg(feature = "sim")]

mod support;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use paw_core::setup::address::StoreAddress;
use paw_core::setup::touch::{TouchClock, TouchExpectation};
use paw_core::setup::outcome::RemoteReason;
use paw_core::setup::{SetupError, SetupEvents, SetupFlow, SetupState};
use paw_core::sim::{SimOptions, SimulatedKey};
use paw_core::ssh::{CardSigner, card_ssh_public_key};
use paw_core::sync::hosts::{self, GITHUB_HOST_KEYS, KnownHosts};
use paw_core::sync::{Connection, HostKeyPin};
use paw_core::write::LineStatus;
use paw_core::{ApduTransport, KeyTouch, card_info};
use support::{GitServer, git};

const PIN: &str = "123456";

fn card(touch: bool) -> Arc<SimulatedKey> {
    SimulatedKey::with_options(SimOptions {
        pin: PIN.into(),
        decrypt_touch: touch,
        auth_touch: touch,
        touch_timeout: Duration::from_millis(100),
        extended_length: false,
    })
}

fn authorised(card: &Arc<SimulatedKey>) -> String {
    card_ssh_public_key(card.clone()).unwrap().authorized_key()
}

/// What the app would do: touch when asked, count the asks, and stop the
/// download at the first progress report if told to.
struct Events {
    card: Arc<SimulatedKey>,
    touch: bool,
    asked: AtomicU32,
    progress: AtomicU32,
    stop: AtomicBool,
}

impl SetupEvents for Events {
    fn touch_needed(&self, _expectation: TouchExpectation) {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if self.touch {
            self.card.touch();
        }
    }

    fn progress(&self, _received: u32, _total: u32) -> bool {
        self.progress.fetch_add(1, Ordering::SeqCst);
        !self.stop.load(Ordering::SeqCst)
    }
}

fn events(card: &Arc<SimulatedKey>, touch: bool) -> Arc<Events> {
    Arc::new(Events {
        card: card.clone(),
        touch,
        asked: AtomicU32::new(0),
        progress: AtomicU32::new(0),
        stop: AtomicBool::new(false),
    })
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

/// A flow at `CheckHost` for `server`, with the key read.
fn at_check_host(dir: &Path, card: &Arc<SimulatedKey>, server: &GitServer) -> SetupFlow {
    let mut flow = SetupFlow::open(dir).unwrap();
    flow.read_key(card.clone()).unwrap();
    flow.set_address(&address(server)).unwrap();
    flow
}

/// A flow at `NeedPin`: the host's key confirmed and the key accepted.
fn at_need_pin(dir: &Path, card: &Arc<SimulatedKey>, server: &GitServer) -> SetupFlow {
    let mut flow = at_check_host(dir, card, server);
    let SetupState::ConfirmHost { fingerprint, .. } = flow.check_host().unwrap() else {
        panic!("expected ConfirmHost");
    };
    assert!(matches!(flow.confirm_host(&fingerprint).unwrap(), SetupState::NeedPin { .. }));
    flow
}

fn download(flow: &mut SetupFlow, card: &Arc<SimulatedKey>, events: &Arc<Events>) -> Result<SetupState, SetupError> {
    flow.download(card.clone(), PIN, events.clone())
}

// --- the whole flow --------------------------------------------------------

#[test]
fn setup_reads_the_key_checks_the_host_downloads_and_opens_one_entry() {
    let card = card(true);
    let server = GitServer::start(&[authorised(&card)]);
    push_fixture_store(&server);
    let dir = tempfile::tempdir().unwrap();
    let events = events(&card, true);
    let mut flow = SetupFlow::open(dir.path()).unwrap();
    assert_eq!(flow.state(), SetupState::ReadKey);

    let SetupState::Address { key } = flow.read_key(card.clone()).unwrap() else { panic!() };
    assert_eq!(key.ssh_public_key, authorised(&card));
    assert_eq!(key.authentication_touch, KeyTouch::EachUse);

    let state = flow.set_address(&address(&server)).unwrap();
    assert_eq!(state, SetupState::CheckHost { address: address(&server), notice: None });

    // An unknown host: its key is shown before anything else happens.
    let SetupState::ConfirmHost { host, algorithm, fingerprint } = flow.check_host().unwrap() else { panic!() };
    assert_eq!((host.as_str(), algorithm.as_str()), ("127.0.0.1", "ssh-ed25519"));
    assert_eq!(fingerprint, server.remote().host_key.fingerprint());

    let state = flow.confirm_host(&fingerprint).unwrap();
    assert_eq!(state, SetupState::NeedPin { address: address(&server), touch: TouchExpectation::Needed });
    // Checking the host and the key cost no signature and no PIN try.
    assert_eq!(card.authentication_count(), 0);
    assert_eq!(server.logins(), 0);

    let state = download(&mut flow, &card, &events).unwrap();
    assert_eq!(state, SetupState::TestDecrypt { entries: 29, touch: TouchExpectation::Needed });
    assert_eq!(card.authentication_count(), 1);
    assert!(events.progress.load(Ordering::SeqCst) > 0);
    assert_eq!(git(&flow.repo_dir(), &["rev-parse", "HEAD"]), server.tip());

    let state = flow.test_decrypt(card.clone(), PIN, events.clone()).unwrap();
    let SetupState::Done { opened, entries } = state else { panic!("{state:?}") };
    assert_eq!(entries, 29);
    assert!(!opened.starts_with("archive/"), "{opened}");
    assert_eq!(card.decipher_count(), 1);
    assert_eq!(events.asked.load(Ordering::SeqCst), 2);
    assert_eq!(card_info(card.clone()).unwrap().pin_tries_left, 3);

    // Nothing secret is saved.
    let saved = std::fs::read_to_string(dir.path().join("setup")).unwrap();
    let fields: Vec<(&str, &str)> = saved.lines().filter_map(|line| line.split_once('=')).collect();
    let names: Vec<&str> = fields.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        ["step", "ident", "ssh_public_key", "decryption_fingerprint", "authentication_touch", "decryption_touch", "address", "opened"]
    );
    // (The simulated key's serial, 12345678, happens to contain its PIN.)
    assert!(fields.iter().all(|(_, value)| *value != PIN), "{saved}");
    assert!(saved.contains("step=done"), "{saved}");
    // Sync can find the store.
    assert_eq!(paw_core::setup::saved_address(dir.path()).unwrap().display(), address(&server));
}

#[test]
fn a_confirmed_host_is_pinned_and_not_asked_about_again() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    let first = tempfile::tempdir().unwrap();
    at_need_pin(first.path(), &card, &server);

    let pinned = KnownHosts::new(&first.path().join("known_hosts"))
        .lookup("127.0.0.1", server.remote().port)
        .unwrap()
        .unwrap();
    assert_eq!(pinned.fingerprint(), server.remote().host_key.fingerprint());

    // Back to CheckHost: now the host is known, so it goes straight on.
    let mut flow = SetupFlow::open(first.path()).unwrap();
    flow.back().unwrap();
    assert!(matches!(flow.check_host().unwrap(), SetupState::NeedPin { .. }));
}

#[test]
fn a_wrong_fingerprint_does_not_pin_the_host() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_check_host(dir.path(), &card, &server);
    flow.check_host().unwrap();

    let error = flow.confirm_host("SHA256:not-what-was-shown").unwrap_err();

    assert!(matches!(error, SetupError::HostKeyNotConfirmed { .. }), "{error:?}");
    assert!(!dir.path().join("known_hosts").exists());
    assert!(matches!(flow.state(), SetupState::ConfirmHost { .. }));
}

// --- each failure ----------------------------------------------------------

#[test]
fn an_unreachable_host_is_its_own_failure() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_check_host(dir.path(), &card, &server);
    drop(server);

    let error = flow.check_host().unwrap_err();

    assert!(matches!(&error, SetupError::HostUnreachable { host, .. } if host == "127.0.0.1"), "{error:?}");
    assert!(matches!(flow.state(), SetupState::CheckHost { .. }));
    assert_eq!(flow.last_remote_message().unwrap().reason, RemoteReason::Unreachable);
}

#[test]
fn a_changed_host_key_stops_with_both_fingerprints() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    let dir = tempfile::tempdir().unwrap();
    let known = KnownHosts::new(&dir.path().join("known_hosts"));
    known.add("127.0.0.1", server.remote().port, &support::some_other_host_key()).unwrap();
    let pinned = HostKeyPin::parse(&support::some_other_host_key()).unwrap().fingerprint();
    let mut flow = at_check_host(dir.path(), &card, &server);

    let error = flow.check_host().unwrap_err();

    match error {
        SetupError::HostKeyChanged { expected, presented, .. } => {
            assert_eq!(expected, [pinned]);
            assert_eq!(presented, server.remote().host_key.fingerprint());
        }
        other => panic!("expected HostKeyChanged, got {other:?}"),
    }
    assert_eq!(server.logins(), 0);
}

#[test]
fn a_key_the_host_does_not_know_is_reported_with_the_line_to_add_before_any_pin() {
    let card = card(true);
    let someone_else = paw_core::ssh::SshPublicKey::ed25519(&[3u8; 32]).authorized_key();
    let server = GitServer::start(&[someone_else]);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_check_host(dir.path(), &card, &server);
    let SetupState::ConfirmHost { fingerprint, .. } = flow.check_host().unwrap() else { panic!() };

    let error = flow.confirm_host(&fingerprint).unwrap_err();

    match error {
        SetupError::KeyNotAccepted { host, public_key } => {
            assert_eq!(host, "127.0.0.1");
            assert_eq!(public_key, authorised(&card));
        }
        other => panic!("expected KeyNotAccepted, got {other:?}"),
    }
    assert_eq!(card.authentication_count(), 0);
    assert_eq!(card_info(card).unwrap().pin_tries_left, 3);
    assert!(matches!(flow.state(), SetupState::CheckHost { .. }));
    assert_eq!(flow.last_remote_message().unwrap().reason, RemoteReason::KeyNotAccepted);
}

#[test]
fn a_repository_the_key_cannot_see_is_its_own_outcome_on_the_one_shot_path() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    push_fixture_store(&server);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_need_pin(dir.path(), &card, &server);
    server.sim.set_repository_not_found(true);

    let error = download(&mut flow, &card, &events(&card, true)).unwrap_err();

    assert!(matches!(&error, SetupError::Failed { message } if message.contains("can't see it")), "{error:?}");
    let message = flow.last_remote_message().unwrap();
    assert_eq!(message.reason, RemoteReason::RepositoryNotFound);
    assert_eq!(message.said.as_deref(), Some("ERROR: Repository not found."));
    assert!(matches!(flow.state(), SetupState::NeedPin { .. }));
}

#[test]
fn the_key_check_asks_without_a_signature() {
    let card = card(true);
    let server = GitServer::start(&[authorised(&card)]);
    let key = card_ssh_public_key(card.clone()).unwrap();

    assert!(Connection::probe(&server.remote(), &key).unwrap());
    assert!(!Connection::probe(&server.remote(), &paw_core::ssh::SshPublicKey::ed25519(&[3u8; 32])).unwrap());
    assert_eq!(card.authentication_count(), 0);
    assert_eq!(server.logins(), 0);
}

#[test]
fn a_wrong_pin_reports_the_tries_left_then_the_block() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    push_fixture_store(&server);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_need_pin(dir.path(), &card, &server);
    let events = events(&card, true);

    for tries_left in [2, 1] {
        let error = flow.download(card.clone(), "000000", events.clone()).unwrap_err();
        assert!(matches!(error, SetupError::WrongPin { tries_left: t } if t == tries_left), "{error:?}");
        assert!(matches!(flow.state(), SetupState::NeedPin { .. }));
    }
    let error = flow.download(card.clone(), "000000", events.clone()).unwrap_err();
    assert!(matches!(error, SetupError::PinBlocked), "{error:?}");
    assert!(!flow.repo_dir().exists());
}

#[test]
fn a_key_nobody_touches_times_out() {
    let card = card(true);
    let server = GitServer::start(&[authorised(&card)]);
    push_fixture_store(&server);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_need_pin(dir.path(), &card, &server);

    let error = download(&mut flow, &card, &events(&card, false)).unwrap_err();

    assert!(matches!(error, SetupError::TouchTimedOut), "{error:?}");
    assert!(matches!(flow.state(), SetupState::NeedPin { .. }));
}

#[test]
fn an_empty_repository_is_its_own_failure() {
    let card = card(false);
    let server = GitServer::start_empty(&[authorised(&card)]);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_need_pin(dir.path(), &card, &server);

    let error = download(&mut flow, &card, &events(&card, true)).unwrap_err();

    assert!(matches!(error, SetupError::EmptyRepository), "{error:?}");
    assert!(!flow.repo_dir().exists());
    // Back at the address, which is kept, with the reason, also after a
    // relaunch.
    assert!(matches!(flow.state(), SetupState::Address { .. }), "{:?}", flow.state());
    let message = flow.last_remote_message().unwrap();
    assert_eq!(message.reason, RemoteReason::EmptyRepository);
    assert_eq!((message.host.as_str(), message.repository.as_str()), ("127.0.0.1", "store.git"));
    let relaunched = SetupFlow::open(dir.path()).unwrap();
    assert!(matches!(relaunched.state(), SetupState::Address { .. }));
    assert_eq!(relaunched.last_remote_message(), Some(message));
    // The same address again, once the store is pushed: it goes through.
    let mut flow = relaunched;
    push_fixture_store(&server);
    flow.set_address(&address(&server)).unwrap();
    assert_eq!(flow.last_remote_message(), None);
    assert!(matches!(flow.check_host().unwrap(), SetupState::NeedPin { .. }));
    assert!(matches!(download(&mut flow, &card, &events(&card, true)).unwrap(), SetupState::TestDecrypt { .. }));
    assert_eq!(SetupFlow::open(dir.path()).unwrap().last_remote_message(), None);
    assert!(!std::fs::read_to_string(dir.path().join("setup")).unwrap().contains("problem="));
}

#[test]
fn a_store_with_a_link_in_it_is_refused_and_says_where() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    push_fixture_store(&server);
    let state = tempfile::tempdir().unwrap();
    server.desktop_symlink("up", Path::new(".."));
    let mut flow = at_need_pin(state.path(), &card, &server);

    let error = download(&mut flow, &card, &events(&card, true)).unwrap_err();

    let SetupError::Failed { message } = &error else { panic!("{error:?}") };
    assert!(message.starts_with("the remote contains something paw will not check out"), "{message}");
    let refused = flow.refused_remote();
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!((refused[0].path.as_str(), refused[0].reason.as_str()), ("up", "it is a symbolic link"));
    assert!(!flow.repo_dir().exists());
    assert!(matches!(flow.state(), SetupState::NeedPin { .. }));
    // A download that succeeds forgets it.
    let desktop = server.desktop();
    git(desktop.path(), &["rm", "--quiet", "up"]);
    git(desktop.path(), &["commit", "--quiet", "-m", "No link."]);
    git(desktop.path(), &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    download(&mut flow, &card, &events(&card, true)).unwrap();
    assert!(flow.refused_remote().is_empty());
}

#[test]
fn a_repository_without_gpg_id_is_not_a_pass_store() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_need_pin(dir.path(), &card, &server);

    let error = download(&mut flow, &card, &events(&card, true)).unwrap_err();

    assert!(matches!(error, SetupError::NotAPassStore), "{error:?}");
    assert!(!flow.repo_dir().exists());
    assert!(matches!(flow.state(), SetupState::Address { .. }), "{:?}", flow.state());
    assert_eq!(flow.last_remote_message().unwrap().reason, RemoteReason::NotAPassStore);
    assert_eq!(SetupFlow::open(dir.path()).unwrap().last_remote_message().unwrap().reason, RemoteReason::NotAPassStore);
    // Going back from the address forgets it.
    flow.back().unwrap();
    assert_eq!(SetupFlow::open(dir.path()).unwrap().last_remote_message(), None);
}

#[test]
fn a_store_the_key_cannot_open_is_reported_before_any_touch() {
    let card = card(true);
    let server = GitServer::start(&[authorised(&card)]);
    push_foreign_store(&server);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_need_pin(dir.path(), &card, &server);
    let events = events(&card, true);
    download(&mut flow, &card, &events).unwrap();

    let error = flow.test_decrypt(card.clone(), PIN, events.clone()).unwrap_err();

    assert!(matches!(error, SetupError::KeyCannotOpenStore), "{error:?}");
    assert_eq!(error.to_string(), "cloned, but this key can't open any entry in the store");
    assert_eq!(card.decipher_count(), 0);
    assert!(matches!(flow.state(), SetupState::TestDecrypt { .. }));
}

#[test]
fn a_step_out_of_order_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut flow = SetupFlow::open(dir.path()).unwrap();

    let error = flow.set_address("git@github.com:you/store.git").unwrap_err();

    assert!(matches!(error, SetupError::WrongStep { .. }), "{error:?}");
}

// --- resuming after a kill --------------------------------------------------

/// What the app sees after being killed and started again.
fn relaunch(dir: &Path) -> SetupState {
    SetupFlow::open(dir).unwrap().state()
}

#[test]
fn setup_resumes_after_a_kill_at_every_step() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    push_fixture_store(&server);
    let dir = tempfile::tempdir().unwrap();
    let events = events(&card, true);
    let mut flow = SetupFlow::open(dir.path()).unwrap();
    assert_eq!(relaunch(dir.path()), SetupState::ReadKey);

    let state = flow.read_key(card.clone()).unwrap();
    assert_eq!(relaunch(dir.path()), state);

    let state = flow.set_address(&address(&server)).unwrap();
    assert_eq!(relaunch(dir.path()), state);

    // Killed while the host's key waits for confirmation: the host is asked
    // again, so the key shown is always the one presented now.
    let confirm = flow.check_host().unwrap();
    assert!(matches!(relaunch(dir.path()), SetupState::CheckHost { .. }));
    let SetupState::ConfirmHost { fingerprint, .. } = confirm else { panic!() };
    let state = flow.confirm_host(&fingerprint).unwrap();
    assert_eq!(relaunch(dir.path()), state);

    // Killed in the middle of the download (the progress report stops it
    // where a kill would): the half-made copy is removed on relaunch.
    events.stop.store(true, Ordering::SeqCst);
    let error = download(&mut flow, &card, &events).unwrap_err();
    assert!(matches!(error, SetupError::Cancelled), "{error:?}");
    // Nothing is reported after the listener said stop.
    assert_eq!(events.progress.load(Ordering::SeqCst), 1);
    drop(flow);
    assert!(dir.path().join("repo").exists());
    let mut flow = SetupFlow::open(dir.path()).unwrap();
    assert!(matches!(flow.state(), SetupState::NeedPin { .. }));
    assert!(!flow.repo_dir().exists());

    events.stop.store(false, Ordering::SeqCst);
    let state = download(&mut flow, &card, &events).unwrap();
    assert_eq!(relaunch(dir.path()), state);
    let logins = server.logins();

    // Killed before the test decrypt: no second download.
    let mut flow = SetupFlow::open(dir.path()).unwrap();
    let state = flow.test_decrypt(card.clone(), PIN, events.clone()).unwrap();
    assert!(matches!(state, SetupState::Done { .. }));
    assert_eq!(relaunch(dir.path()), state);
    assert_eq!(server.logins(), logins);
}

#[test]
fn a_store_that_went_missing_after_the_download_is_downloaded_again() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    push_fixture_store(&server);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_need_pin(dir.path(), &card, &server);
    download(&mut flow, &card, &events(&card, true)).unwrap();
    std::fs::remove_dir_all(flow.repo_dir()).unwrap();

    assert!(matches!(relaunch(dir.path()), SetupState::NeedPin { .. }));
}

#[test]
fn an_unreadable_state_file_starts_setup_over() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("setup"), "step=done\n").unwrap();

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

/// The simulated key, reporting a cached touch policy for its
/// authentication slot (whose own policy is off, so it signs untouched,
/// as a key with a fresh cached touch does).
struct CachedPolicy(Arc<SimulatedKey>);

impl ApduTransport for CachedPolicy {
    fn transmit(&self, apdu: &[u8]) -> Result<Vec<u8>, String> {
        if apdu.starts_with(&[0x00, 0xCA, 0x00, 0xD8]) {
            return Ok(vec![0x03, 0x20, 0x90, 0x00]);
        }
        let mut response = self.0.exchange(apdu.to_vec());
        if apdu.starts_with(&[0x00, 0xCA, 0x00, 0x6E])
            && let Some(at) = response.windows(4).position(|w| w == [0xD8, 0x02, 0x00, 0x20])
        {
            response[at + 2] = 0x03;
        }
        Ok(response)
    }
}

#[test]
fn a_second_login_within_the_cached_window_does_not_ask_for_a_touch() {
    let sim = card(false);
    let cached: Arc<dyn ApduTransport> = Arc::new(CachedPolicy(sim.clone()));
    assert_eq!(card_info(cached.clone()).unwrap().authentication_touch, KeyTouch::Cached);
    let server = GitServer::start(&[authorised(&sim)]);
    server.sim.set_repository_not_found(true);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = SetupFlow::open(dir.path()).unwrap();
    flow.read_key(cached.clone()).unwrap();
    flow.set_address(&address(&server)).unwrap();
    let SetupState::ConfirmHost { fingerprint, .. } = flow.check_host().unwrap() else { panic!() };
    flow.confirm_host(&fingerprint).unwrap();
    let events = events(&sim, false);

    // The first login asks; the repository is not there, so it is tried again.
    let first = flow.download(cached.clone(), PIN, events.clone()).unwrap_err();
    assert!(matches!(first, SetupError::Failed { .. }), "{first:?}");
    assert_eq!(events.asked.load(Ordering::SeqCst), 1);
    assert_eq!(flow.state(), SetupState::NeedPin { address: address(&server), touch: TouchExpectation::NotNeeded });

    flow.download(cached.clone(), PIN, events.clone()).unwrap_err();
    assert_eq!(events.asked.load(Ordering::SeqCst), 1);
    assert_eq!(sim.authentication_count(), 2);
}

#[test]
fn a_key_that_needs_a_touch_each_time_is_asked_each_time() {
    let card = card(true);
    let server = GitServer::start(&[authorised(&card)]);
    server.sim.set_repository_not_found(true);
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_need_pin(dir.path(), &card, &server);
    let events = events(&card, true);

    download(&mut flow, &card, &events).unwrap_err();
    download(&mut flow, &card, &events).unwrap_err();

    assert_eq!(events.asked.load(Ordering::SeqCst), 2);
}

// --- addresses -------------------------------------------------------------

#[test]
fn addresses_are_read_in_the_forms_git_hosts_show() {
    let ok = |text: &str, user: &str, host: &str, port: u16, path: &str| {
        let parsed = StoreAddress::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
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
fn a_github_web_address_becomes_its_ssh_address_and_says_so() {
    for text in [
        "https://github.com/you/password-store",
        "https://github.com/you/password-store.git",
        "https://github.com/you/password-store/",
        "http://www.github.com/you/password-store",
    ] {
        let parsed = StoreAddress::parse(text).unwrap();
        assert_eq!(parsed.address.display(), "git@github.com:you/password-store.git", "{text}");
        let notice = parsed.notice.unwrap();
        assert!(notice.contains("git@github.com:you/password-store.git"), "{notice}");
    }
}

#[test]
fn addresses_paw_cannot_use_are_refused_with_what_to_do() {
    for (text, says) in [
        ("", "Type or paste"),
        ("git@github.com:you/my store.git", "no spaces"),
        ("https://gitlab.com/you/store", "connects over SSH"),
        ("https://github.com/you/store/tree/main", "repository itself"),
        ("git://github.com/you/store.git", "connects over SSH"),
        ("file:///home/you/store", "connects over SSH"),
        ("github.com:you/store.git", "user name"),
        ("/home/you/store", "doesn't look like a git address"),
        ("C:/Users/you/store", "user name"),
        ("git@github.com:", "after the colon"),
        ("ssh://github.com/you/store.git", "user name"),
        ("ssh://git@github.com:99999/you/store.git", "port"),
        ("ssh://git@github.com", "path"),
        ("git@-oProxyCommand=x:store", "doesn't look like"),
        ("git@github.com:-u", "after the colon"),
    ] {
        let message = StoreAddress::parse(text).err().unwrap_or_else(|| panic!("{text:?} was accepted"));
        assert!(message.contains(says), "{text:?}: {message}");
    }
}

// --- host keys --------------------------------------------------------------

#[test]
fn githubs_bundled_keys_match_their_published_fingerprints() {
    for (line, fingerprint) in GITHUB_HOST_KEYS {
        assert_eq!(HostKeyPin::parse(line).unwrap().fingerprint(), fingerprint);
    }
    let github = hosts::bundled("github.com", 22).unwrap();
    assert_eq!(github.fingerprints(), GITHUB_HOST_KEYS.map(|(_, fp)| fp.to_owned()));
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
    let card = card(false);
    let server = GitServer::start_with_two_host_keys(&[authorised(&card)]);
    let signer = CardSigner::new(card.clone(), PIN, || {}).unwrap();

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
    let known = KnownHosts::new(&dir.path().join("known_hosts"));
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
    paw_core::encrypt_to_keys(rand::thread_rng(), keys, text.as_bytes()).unwrap()
}

/// Sets up `server`'s store with `card` and returns the state directory.
fn set_up(card: &Arc<SimulatedKey>, server: &GitServer) -> tempfile::TempDir {
    let state = tempfile::tempdir().unwrap();
    let events = events(card, false);
    let mut flow = at_need_pin(state.path(), card, server);
    download(&mut flow, card, &events).unwrap();
    let done = flow.test_decrypt(card.clone(), PIN, events.clone()).unwrap();
    assert!(matches!(done, SetupState::Done { .. }), "{done:?}");
    state
}

#[test]
fn a_team_line_at_the_top_is_not_bound_to_the_card_that_opened_a_personal_entry() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    let mine = paw_core::card_decryption_key(card.clone()).unwrap();
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

    let state = set_up(&card, &server);

    let flow = SetupFlow::open(state.path()).unwrap();
    let keyring = paw_core::write::Keyring::open(state.path()).unwrap();
    assert_eq!(keyring.line_status("team@example.com", 0), LineStatus::Missing);
    assert!(matches!(keyring.line_status("owner@example.com", 0), LineStatus::Ready { bound: true, .. }));
    let readiness = flow.write_readiness().unwrap();
    assert!(!readiness.can_save);
    assert_eq!(readiness.missing_recipients, ["team@example.com"]);
    assert!(readiness.folders_needing_keys.is_empty(), "{readiness:?}");
}

#[test]
fn the_one_line_of_the_gpg_id_that_governs_the_opened_entry_is_bound() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    let mine = paw_core::card_decryption_key(card.clone()).unwrap();
    push_files(
        &server,
        &[(".gpg-id", b"owner@example.com\n".to_vec()), ("bank.gpg", encrypted_for(&[mine], "mine\n"))],
    );

    let state = set_up(&card, &server);

    let keyring = paw_core::write::Keyring::open(state.path()).unwrap();
    assert!(matches!(keyring.line_status("owner@example.com", 0), LineStatus::Ready { bound: true, .. }));
    let readiness = SetupFlow::open(state.path()).unwrap().write_readiness().unwrap();
    assert!(readiness.can_save, "{readiness:?}");
}

#[test]
fn nothing_is_bound_when_the_opened_entry_does_not_name_the_card() {
    let card = card(false);
    let server = GitServer::start(&[authorised(&card)]);
    let mine = paw_core::card_decryption_key(card.clone()).unwrap();
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

    let state = set_up(&card, &server);

    let keyring = paw_core::write::Keyring::open(state.path()).unwrap();
    assert_eq!(keyring.line_status("owner@example.com", 0), LineStatus::Missing);
    let readiness = SetupFlow::open(state.path()).unwrap().write_readiness().unwrap();
    assert_eq!(readiness.missing_recipients, ["owner@example.com"]);
}

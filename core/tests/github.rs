//! GitHub sign-in, the repository picker and the read-only HTTPS refresh,
//! against the simulated GitHub (`sim::github`) in front of the simulated
//! SSH host (`sim::remote`). Nothing here contacts the real GitHub, and no
//! test sleeps for a poll interval: the device flow's clock is simulated.

#![cfg(feature = "sim")]

mod key_common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use paw_core::github::{
    BackgroundOutcome, GitHubAccount, GitHubError, RefreshDecision, RefreshStep, RepositoryList, SignInState,
};
use paw_core::ffi::sync::Setup;
use paw_core::key::{KeyOutcome, KeyTaskState};
use paw_core::setup::SetupState;
use paw_core::sim::SimulatedKey;
use paw_core::sim::github::{SIM_USER_CODE, SimBystander, SimDeviceOutcome, SimGitHub, SimPollClock, SimTokenVault};
use paw_core::sim::remote::SimRemote;
use paw_core::sync::operation::{self, SyncState};
use paw_core::sync::{self, SyncError};

use key_common::{Driver, finish, port_with, signer_with};

const PIN: &str = "123456";
const REPOSITORY: &str = "you/password-store";
const NEW_ENTRY: &str = "web/new-site.gpg";

struct World {
    key: Arc<SimulatedKey>,
    remote: Arc<SimRemote>,
    github: Arc<SimGitHub>,
    vault: Arc<SimTokenVault>,
    clock: Arc<SimPollClock>,
    wall: Arc<AtomicU64>,
    dir: tempfile::TempDir,
    account: Arc<GitHubAccount>,
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn world() -> World {
    let key = SimulatedKey::new(PIN.into(), false, 1000);
    let remote = SimRemote::start(key.clone()).unwrap();
    let github = SimGitHub::start_for(&remote, REPOSITORY).unwrap();
    let vault = SimTokenVault::new();
    let clock = SimPollClock::new();
    let wall = Arc::new(AtomicU64::new(unix_now()));
    let dir = tempfile::tempdir().unwrap();
    let account = Arc::new(account_for(&github, dir.path(), &vault, &clock, &wall));
    World { key, remote, github, vault, clock, wall, dir, account }
}

fn account_for(
    github: &SimGitHub,
    dir: &Path,
    vault: &Arc<SimTokenVault>,
    clock: &Arc<SimPollClock>,
    wall: &Arc<AtomicU64>,
) -> GitHubAccount {
    let wall = wall.clone();
    GitHubAccount::with(github.endpoints(), dir, vault.clone(), clock.clone(), Arc::new(move || wall.load(Ordering::SeqCst)))
}

impl World {
    fn state_dir(&self) -> &Path {
        self.dir.path()
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn sign_in(&self) {
        let sign_in = self.account.start_sign_in().unwrap();
        for _ in 0..20 {
            match sign_in.poll().unwrap() {
                SignInState::Waiting => continue,
                SignInState::SignedIn => return,
                other => panic!("sign-in ended {other:?}"),
            }
        }
        panic!("sign-in never finished");
    }

    /// Signs in, lists, picks the store, and runs setup with the picked
    /// address up to the clone, over SSH with the key.
    fn set_up(&self) {
        self.sign_in();
        let RepositoryList::Repositories { repositories, .. } = self.account.repositories().unwrap() else {
            panic!("not installed");
        };
        assert_eq!(repositories[0].full_name, REPOSITORY);
        let address = self.account.select_repository(REPOSITORY).unwrap();
        let port = port_with(&self.key);
        let setup = Setup::open(self.state_dir().to_string_lossy().into_owned()).unwrap();
        setup.read_key_from_session(port.session().unwrap()).unwrap();
        setup.set_address(address).unwrap();
        let SetupState::ConfirmHost { fingerprint, .. } = setup.check_host().unwrap() else {
            panic!("expected ConfirmHost");
        };
        setup.confirm_host(fingerprint).unwrap();
        let driver = Driver::helpful(PIN, &self.key);
        let task = setup.clone().download_with_key(port, driver.clone());
        driver.drive(&task);
        let done = finish(&task);
        assert!(matches!(done, KeyTaskState::Done { outcome: KeyOutcome::SignedIn }), "{done:?}");
        let state = setup.state();
        assert!(matches!(state, SetupState::TestDecrypt { .. }), "{state:?}");
    }

    fn head(&self) -> String {
        git2::Repository::open(self.repo()).unwrap().head().unwrap().target().unwrap().to_string()
    }

    /// The working copy matches the branch, and nothing is half done.
    fn assert_consistent(&self) {
        let repo = git2::Repository::open(self.repo()).unwrap();
        let mut options = git2::StatusOptions::new();
        options.include_untracked(true);
        let statuses = repo.statuses(Some(&mut options)).unwrap();
        let dirty: Vec<String> = statuses.iter().filter_map(|s| s.path().ok().map(str::to_owned)).collect();
        assert!(dirty.is_empty(), "working copy differs from the branch: {dirty:?}");
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
    }

    /// Every token the simulated GitHub ever issued.
    fn tokens(&self) -> Vec<String> {
        self.github.all_tokens()
    }

    fn git_requests(&self) -> usize {
        self.github.requests().iter().filter(|r| r.path.contains(".git/")).count()
    }
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// A commit on the remote's `main` adding a symbolic link.
fn push_link(remote: &SimRemote) {
    let repo = git2::Repository::open_bare(remote.bare()).unwrap();
    let parent = repo.find_commit(repo.refname_to_id("refs/heads/main").unwrap()).unwrap();
    let mut tree = repo.treebuilder(Some(&parent.tree().unwrap())).unwrap();
    let target = repo.blob(b"../../etc").unwrap();
    tree.insert("escape", target, 0o120000).unwrap();
    let tree = repo.find_tree(tree.write().unwrap()).unwrap();
    let who = git2::Signature::now("Desktop", "desktop@example.com").unwrap();
    repo.commit(Some("refs/heads/main"), &who, &who, "A link.", &tree, &[&parent]).unwrap();
}

/// Replaces the remote's `main` with an unrelated history.
fn replace_history(remote: &SimRemote) {
    let repo = git2::Repository::open_bare(remote.bare()).unwrap();
    let blob = repo.blob(b"0123456789ABCDEF\n").unwrap();
    let mut tree = repo.treebuilder(None).unwrap();
    tree.insert(".gpg-id", blob, 0o100644).unwrap();
    let tree = repo.find_tree(tree.write().unwrap()).unwrap();
    let who = git2::Signature::now("Desktop", "desktop@example.com").unwrap();
    let id = repo.commit(None, &who, &who, "Start again.", &tree, &[]).unwrap();
    repo.reference("refs/heads/main", id, true, "replace").unwrap();
}

/// Every file under `dir`, recursively, with its bytes.
fn files_under(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                folders.push(entry.path());
            } else if kind.is_file() {
                found.push((entry.path(), std::fs::read(entry.path()).unwrap()));
            }
        }
    }
    found
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// A second clone of the same remote over SSH, for comparing results.
fn ssh_twin(w: &World) -> tempfile::TempDir {
    let twin = tempfile::tempdir().unwrap();
    let remote = w.remote.remote().unwrap();
    let signer = signer_with(&w.key, PIN);
    let connection = sync::Connection::open(&remote, &signer).unwrap();
    sync::clone(&connection, &remote, &twin.path().join("repo")).unwrap();
    twin
}

fn ssh_refresh(w: &World, twin: &Path) -> Result<operation::SyncReport, SyncError> {
    let signer = signer_with(&w.key, PIN);
    operation::refresh(&w.remote.remote().unwrap(), &signer, &twin.join("repo"))
}

// --- the device flow ------------------------------------------------------------

#[test]
fn sign_in_polls_at_githubs_interval_until_the_person_authorises() {
    let w = world();
    w.github.set_device_outcome(SimDeviceOutcome::AuthoriseAfter { pending: 2 });

    let sign_in = w.account.start_sign_in().unwrap();
    let code = sign_in.code();
    assert_eq!(code.user_code, SIM_USER_CODE);
    assert_eq!(code.verification_uri, format!("{}/login/device", w.github.url()));
    assert_eq!((code.interval, code.expires_in), (5, 900));

    assert_eq!(sign_in.poll().unwrap(), SignInState::Waiting);
    assert_eq!(sign_in.poll().unwrap(), SignInState::Waiting);
    assert_eq!(sign_in.poll().unwrap(), SignInState::SignedIn);
    // Once over, it stays over and asks nothing more.
    assert_eq!(sign_in.poll().unwrap(), SignInState::SignedIn);

    assert_eq!(w.clock.waits(), vec![secs(5), secs(10), secs(15)]);
    assert_eq!(sign_in.polls(), 3);
    assert!(w.account.signed_in().unwrap());
    assert!(!format!("{sign_in:?}").contains("ghu_"));
}

#[test]
fn slow_down_adds_five_seconds_to_every_later_poll() {
    let w = world();
    w.github.set_device_outcome(SimDeviceOutcome::SlowDownThenAuthorise { pending: 1 });
    let sign_in = w.account.start_sign_in().unwrap();

    assert_eq!(sign_in.poll().unwrap(), SignInState::Waiting); // slow_down
    assert_eq!(sign_in.interval(), secs(10));
    assert_eq!(sign_in.poll().unwrap(), SignInState::Waiting);
    assert_eq!(sign_in.poll().unwrap(), SignInState::SignedIn);

    assert_eq!(w.clock.waits(), vec![secs(5), secs(15), secs(25)]);
}

#[test]
fn a_person_who_cancels_at_github_ends_the_sign_in_with_nothing_kept() {
    let w = world();
    w.github.set_device_outcome(SimDeviceOutcome::DenyAfter { pending: 1 });
    let sign_in = w.account.start_sign_in().unwrap();

    assert_eq!(sign_in.poll().unwrap(), SignInState::Waiting);
    assert_eq!(sign_in.poll().unwrap(), SignInState::Denied);
    let asked = w.github.request_count();
    assert_eq!(sign_in.poll().unwrap(), SignInState::Denied);
    assert_eq!(w.github.request_count(), asked);
    assert!(w.vault.is_empty());
}

#[test]
fn a_code_github_says_expired_ends_the_sign_in() {
    let w = world();
    w.github.set_device_outcome(SimDeviceOutcome::ExpireAfter { pending: 0 });
    let sign_in = w.account.start_sign_in().unwrap();
    assert_eq!(sign_in.poll().unwrap(), SignInState::Expired);
    assert!(w.vault.is_empty());
}

#[test]
fn a_code_past_its_lifetime_expires_without_asking_github() {
    let w = world();
    w.github.set_timing(5, 12);
    w.github.set_device_outcome(SimDeviceOutcome::WaitForPerson);
    let sign_in = w.account.start_sign_in().unwrap();

    assert_eq!(sign_in.poll().unwrap(), SignInState::Waiting); // at 5
    assert_eq!(sign_in.poll().unwrap(), SignInState::Waiting); // at 10
    let asked = w.github.request_count();
    assert_eq!(sign_in.poll().unwrap(), SignInState::Expired); // at 12
    assert_eq!(w.github.request_count(), asked);
    assert_eq!(w.clock.waits(), vec![secs(5), secs(10), secs(12)]);
}

#[test]
fn the_person_authorising_while_paw_waits_signs_in() {
    let w = world();
    w.github.set_device_outcome(SimDeviceOutcome::WaitForPerson);
    let sign_in = w.account.start_sign_in().unwrap();
    assert_eq!(sign_in.poll().unwrap(), SignInState::Waiting);
    assert!(w.github.has_pending_code());
    w.github.authorise_pending();
    assert_eq!(sign_in.poll().unwrap(), SignInState::SignedIn);
}

#[test]
fn a_cancelled_sign_in_stops_waiting_and_keeps_nothing() {
    let w = world();
    w.github.set_device_outcome(SimDeviceOutcome::AuthoriseAfter { pending: 0 });
    let sign_in = w.account.start_sign_in().unwrap();
    let before = w.github.request_count();
    sign_in.cancel();
    assert_eq!(sign_in.poll().unwrap(), SignInState::Cancelled);
    assert_eq!(sign_in.poll().unwrap(), SignInState::Cancelled);
    assert_eq!(w.github.request_count(), before);
    assert!(w.vault.is_empty());
}

#[test]
fn a_cancel_from_another_thread_wakes_a_real_wait_at_once() {
    let w = world();
    let account = GitHubAccount::with(
        w.github.endpoints(),
        w.state_dir(),
        w.vault.clone(),
        Arc::new(paw_core::github::SystemPollClock::default()),
        paw_core::github::session::system_wall_clock(),
    );
    let sign_in = Arc::new(account.start_sign_in().unwrap());
    let waiting = sign_in.clone();
    let started = std::time::Instant::now();
    let poller = std::thread::spawn(move || waiting.poll().unwrap());
    std::thread::sleep(Duration::from_millis(50));
    sign_in.cancel();
    assert_eq!(poller.join().unwrap(), SignInState::Cancelled);
    assert!(started.elapsed() < secs(4), "the five-second wait was not cut short");
}

#[test]
fn device_sign_in_turned_off_in_the_app_says_so() {
    let w = world();
    w.github.set_device_outcome(SimDeviceOutcome::Disabled);
    assert_eq!(w.account.start_sign_in().unwrap_err(), GitHubError::DeviceFlowDisabled);
}

#[test]
fn signing_in_sends_the_client_id_and_device_code_only_and_says_paw() {
    let w = world();
    w.sign_in();
    let requests = w.github.requests();
    assert_eq!(requests.len(), 2);
    let start = String::from_utf8_lossy(&requests[0].body).into_owned();
    assert_eq!(start, format!("client_id={}", w.github.client_id()));
    let poll = String::from_utf8_lossy(&requests[1].body).into_owned();
    let fields: Vec<&str> = poll.split('&').map(|f| f.split('=').next().unwrap()).collect();
    assert_eq!(fields, vec!["client_id", "device_code", "grant_type"]);
    for request in &requests {
        assert_eq!(request.header("user-agent"), Some("paw"));
        assert!(request.header("authorization").is_none());
        assert!(request.header("cookie").is_none());
    }
}

#[test]
fn the_real_client_id_is_the_apps_and_the_simulation_can_tell_it_apart() {
    assert_eq!(paw_core::github::CLIENT_ID, "Iv23liyBePbnvQPCcGRz");
    let w = world();
    assert_ne!(w.github.endpoints().client_id(), paw_core::github::CLIENT_ID);
    assert_eq!(
        paw_core::github::Endpoints::github().install_url(),
        "https://github.com/apps/paw-password-store/installations/new"
    );
}

// --- the picker ----------------------------------------------------------------

#[test]
fn the_picker_lists_every_granted_repository_across_pages_with_the_store_first() {
    let w = world();
    w.github.set_extra_repositories(250);
    w.sign_in();

    let RepositoryList::Repositories { repositories, install_url } = w.account.repositories().unwrap() else {
        panic!("not installed");
    };
    assert_eq!(repositories.len(), 251);
    assert!(install_url.ends_with("/apps/paw-password-store/installations/new"));
    let store = &repositories[0];
    assert_eq!(store.full_name, REPOSITORY);
    assert_eq!((store.owner.as_str(), store.name.as_str()), ("you", "password-store"));
    assert!(store.private);
    assert_eq!(store.default_branch, "main");
    assert_eq!(store.pushed_at.as_deref(), Some("2026-11-20T09:12:00Z"));
    assert_eq!(store.ssh_address, w.remote.address());
    // Three pages of repositories, and nothing read from inside any of them.
    let pages = w.github.requests().iter().filter(|r| r.path.ends_with("/repositories")).count();
    assert_eq!(pages, 3);
    assert_eq!(w.github.store_probes(), 0);
    assert!(!w.github.requests().iter().any(|r| r.path.contains("/contents/")));
}

#[test]
fn with_the_app_installed_nowhere_the_picker_offers_the_install_page() {
    let w = world();
    w.github.set_installed(false);
    w.sign_in();
    assert_eq!(
        w.account.repositories().unwrap(),
        RepositoryList::NotInstalled { install_url: format!("{}/apps/paw-password-store/installations/new", w.github.url()) }
    );
}

#[test]
fn a_repository_not_in_the_list_cannot_be_chosen() {
    let w = world();
    w.sign_in();
    w.account.repositories().unwrap();
    assert!(matches!(w.account.select_repository("someone/else"), Err(GitHubError::Failed { .. })));
    assert!(w.account.link().is_none());
}

#[test]
fn with_the_vault_empty_nothing_is_sent_anywhere() {
    let w = world();
    assert!(!w.account.signed_in().unwrap());
    assert_eq!(w.account.repositories().unwrap_err(), GitHubError::NotSignedIn);
    assert_eq!(w.account.background_refresh(secs(25)).outcome, BackgroundOutcome::NotSignedIn);
    assert!(w.account.refresh_with_token().is_err());
    assert!(!w.account.sign_out(true).unwrap().revoked);
    assert_eq!(w.github.request_count(), 0);
}

// --- the whole path: sign in, pick, clone with the key, refresh without it -----

#[test]
fn sign_in_pick_clone_over_ssh_then_refresh_over_https_with_no_key() {
    let w = world();
    w.set_up();
    assert_eq!(w.remote.logins(), 1);
    assert_eq!(w.head(), w.remote.tip().unwrap());

    let changed = w.remote.commit_file(NEW_ENTRY.into(), b"ciphertext".to_vec(), "Add web/new-site.".into()).unwrap();
    let report = w.account.refresh_with_token().unwrap();

    assert_eq!(report.state, SyncState::UpToDate);
    assert_eq!(report.integration.fast_forwarded, 1);
    assert_eq!(report.signatures, 0);
    assert_eq!(w.head(), changed);
    assert!(w.repo().join(NEW_ENTRY).is_file());
    // No SSH login, so no card: the key was used once, for the clone.
    assert_eq!(w.remote.logins(), 1);
    assert_eq!(w.key.authentication_count(), 1);
    let git: Vec<_> = w.github.requests().into_iter().filter(|r| r.path.contains(".git/")).collect();
    assert_eq!(git[0].path, format!("/{REPOSITORY}.git/info/refs"));
    assert_eq!(git[0].query, "service=git-upload-pack");
    assert!(git.iter().skip(1).all(|r| r.method == "POST" && r.path.ends_with("/git-upload-pack")));
    assert!(git.iter().all(|r| r.header("authorization").is_some_and(|a| a.starts_with("Basic "))));
    w.assert_consistent();
}

#[test]
fn the_token_is_nowhere_on_disk_in_git_config_or_in_any_message() {
    let w = world();
    w.set_up();
    w.remote.commit_file(NEW_ENTRY.into(), b"x".to_vec(), "Add.".into()).unwrap();
    let report = w.account.refresh_with_token().unwrap();
    w.github.set_redirect_git(Some("http://127.0.0.1:9".into()));
    let error = w.account.refresh_with_token().unwrap_err();
    let background = w.account.background_refresh(secs(25));
    w.github.set_redirect_git(None);

    let tokens = w.tokens();
    assert!(tokens.len() >= 2, "{tokens:?}");
    for (path, bytes) in files_under(w.state_dir()) {
        for token in &tokens {
            assert!(!contains(&bytes, token), "a token is in {}", path.display());
        }
    }
    let config = std::fs::read_to_string(w.repo().join(".git/config")).unwrap();
    assert!(!config.contains("x-access-token") && !config.contains("https://"), "{config}");
    let shown = format!("{report:?} {error} {error:?} {background:?} {:?}", w.account.link());
    for token in &tokens {
        assert!(!shown.contains(token.as_str()));
    }
    // The vault holds them: it is the only place.
    let kept: Vec<u8> = w.vault.items().into_values().flatten().collect();
    assert!(tokens.iter().any(|t| contains(&kept, t)));
}

// --- the token's life --------------------------------------------------------------

#[test]
fn an_expired_access_token_is_refreshed_rotated_and_kept_before_use() {
    let w = world();
    w.set_up();
    let stores = w.vault.store_count();
    w.wall.fetch_add(9 * 3600, Ordering::SeqCst);

    w.remote.commit_file(NEW_ENTRY.into(), b"x".to_vec(), "Add.".into()).unwrap();
    w.account.refresh_with_token().unwrap();

    assert_eq!(w.vault.store_count(), stores + 1);
    assert_eq!(w.github.live_tokens().len(), 2, "the old pair was retired");
    let kept: Vec<u8> = w.vault.items().into_values().flatten().collect();
    assert!(w.github.live_tokens().iter().all(|t| contains(&kept, t)));
    assert!(w.repo().join(NEW_ENTRY).is_file());
}

#[test]
fn an_access_token_github_stopped_accepting_is_refreshed_once_and_the_fetch_retried() {
    let w = world();
    w.set_up();
    w.github.expire_access_tokens();
    w.remote.commit_file(NEW_ENTRY.into(), b"x".to_vec(), "Add.".into()).unwrap();
    w.account.refresh_with_token().unwrap();
    assert!(w.repo().join(NEW_ENTRY).is_file());
}

#[test]
fn killed_after_github_rotated_but_before_the_keychain_write_ends_the_sign_in_cleanly() {
    let w = world();
    w.set_up();
    let head = w.head();
    w.wall.fetch_add(9 * 3600, Ordering::SeqCst);
    w.vault.fail_next_store();

    let first = w.account.refresh_with_token().unwrap_err();
    assert!(first.to_string().contains("could not be kept"), "{first}");
    // The old pair is still kept; GitHub no longer takes it.
    assert!(w.account.signed_in().unwrap());

    let report = w.account.background_refresh(secs(25));
    assert_eq!(report.outcome, BackgroundOutcome::SignedOut);
    assert!(!w.account.signed_in().unwrap());
    assert_eq!(w.head(), head);
    w.assert_consistent();
}

#[test]
fn killed_after_the_keychain_write_the_new_pair_is_used_next_time() {
    let w = world();
    w.set_up();
    w.wall.fetch_add(9 * 3600, Ordering::SeqCst);
    // Stops right after the token step: the refresh happened and was kept.
    let stopped = w.account.background_refresh_with(secs(25), &|step| step == RefreshStep::Fetch);
    assert_eq!(stopped.outcome, BackgroundOutcome::OutOfTime);
    let issued = w.tokens().len();

    w.remote.commit_file(NEW_ENTRY.into(), b"x".to_vec(), "Add.".into()).unwrap();
    assert_eq!(w.account.background_refresh(secs(25)).outcome, BackgroundOutcome::Updated);
    assert_eq!(w.tokens().len(), issued, "no second refresh");
}

#[test]
fn killed_before_the_refresh_request_the_old_pair_still_works() {
    let w = world();
    w.set_up();
    w.wall.fetch_add(9 * 3600, Ordering::SeqCst);
    let stopped = w.account.background_refresh_with(secs(25), &|step| step == RefreshStep::Token);
    assert_eq!(stopped.outcome, BackgroundOutcome::OutOfTime);
    w.remote.commit_file(NEW_ENTRY.into(), b"x".to_vec(), "Add.".into()).unwrap();
    assert_eq!(w.account.background_refresh(secs(25)).outcome, BackgroundOutcome::Updated);
}

#[test]
fn concurrent_refreshes_spend_the_refresh_token_once() {
    let w = world();
    w.set_up();
    w.wall.fetch_add(9 * 3600, Ordering::SeqCst);
    let issued = w.tokens().len();
    // All four start together, so their fetches and integrates overlap
    // (the store lock serialises the ref updates; see tests/store_lock.rs
    // for this race run 200 times).
    let start = Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let (account, start) = (w.account.clone(), start.clone());
            std::thread::spawn(move || {
                start.wait();
                account.refresh_with_token()
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap().unwrap();
    }
    assert_eq!(w.tokens().len(), issued + 2, "exactly one rotation");
}

#[test]
fn a_revoked_sign_in_ends_cleanly_and_leaves_the_store_alone() {
    let w = world();
    w.set_up();
    let head = w.head();
    w.remote.commit_file(NEW_ENTRY.into(), b"x".to_vec(), "Add.".into()).unwrap();
    w.github.revoke_authorisation();

    let error = w.account.refresh_with_token().unwrap_err();
    assert!(matches!(&error, SyncError::Failed { message } if *message == GitHubError::SignedOut.to_string()), "{error}");
    assert!(!w.account.signed_in().unwrap());
    assert_eq!(w.head(), head);
    w.assert_consistent();
    assert_eq!(w.account.background_refresh(secs(25)).outcome, BackgroundOutcome::NotSignedIn);
}

#[test]
fn signing_out_forgets_the_tokens_and_revokes_them_when_asked() {
    let w = world();
    w.sign_in();
    let report = w.account.sign_out(true).unwrap();
    assert!(report.revoked);
    assert_eq!(report.manage_url, format!("{}/settings/apps/authorizations", w.github.url()));
    assert!(w.vault.is_empty());
    assert_eq!(w.github.revoked_tokens().len(), 2);
    assert!(w.github.live_tokens().is_empty());
    let revoke = w.github.requests().into_iter().find(|r| r.path == "/credentials/revoke").unwrap();
    assert!(revoke.header("authorization").is_none());

    let w = world();
    w.sign_in();
    let before = w.github.request_count();
    assert!(!w.account.sign_out(false).unwrap().revoked);
    assert_eq!(w.github.request_count(), before);
    assert!(w.vault.is_empty());
}

// --- the same checks as SSH ------------------------------------------------------------

#[test]
fn a_hostile_tree_fetched_over_https_is_refused_exactly_as_over_ssh() {
    let w = world();
    w.set_up();
    let twin = ssh_twin(&w);
    let head = w.head();
    push_link(&w.remote);

    let over_https = w.account.refresh_with_token().unwrap_err();
    let over_ssh = ssh_refresh(&w, twin.path()).unwrap_err();

    assert!(matches!((&over_https, &over_ssh), (SyncError::Failed { .. }, SyncError::Failed { .. })));
    assert_eq!(over_https.to_string(), over_ssh.to_string());
    assert!(over_https.to_string().starts_with("the remote contains something paw will not check out"), "{over_https}");
    assert_eq!(sync::refused_remote(&w.repo()).unwrap(), sync::refused_remote(&twin.path().join("repo")).unwrap());
    assert_eq!(w.head(), head);
    assert!(!w.repo().join("escape").exists());
    w.assert_consistent();

    let background = w.account.background_refresh(secs(25));
    assert_eq!(background.outcome, BackgroundOutcome::NeedsDecision);
    assert!(matches!(&background.decisions[..], [RefreshDecision::RefusedRemote { paths }] if paths[0].path == "escape"));
    assert_eq!(w.head(), head);
}

#[test]
fn changed_recipients_are_reported_as_over_ssh_and_left_for_the_person_in_the_background() {
    let w = world();
    w.set_up();
    let twin = ssh_twin(&w);
    let head = w.head();
    w.remote.commit_file(".gpg-id".into(), b"0123456789ABCDEF\nFEDCBA9876543210\n".to_vec(), "Share.".into()).unwrap();

    let background = w.account.background_refresh(secs(25));
    assert_eq!(background.outcome, BackgroundOutcome::NeedsDecision);
    let [RefreshDecision::RecipientsChanged { changes }] = &background.decisions[..] else {
        panic!("{background:?}");
    };
    assert_eq!(w.head(), head, "nothing integrated in the background");

    let over_https = w.account.refresh_with_token().unwrap();
    let over_ssh = ssh_refresh(&w, twin.path()).unwrap();
    assert_eq!(over_https.integration.recipients_changed, over_ssh.integration.recipients_changed);
    assert_eq!(&over_https.integration.recipients_changed, changes);
    assert_eq!(over_https.state, over_ssh.state);
}

#[test]
fn a_replaced_history_is_parked_in_the_background_and_reported_as_over_ssh() {
    let w = world();
    w.set_up();
    let twin = ssh_twin(&w);
    let head = w.head();
    replace_history(&w.remote);

    let background = w.account.background_refresh(secs(25));
    assert_eq!(background.outcome, BackgroundOutcome::NeedsDecision);
    assert!(matches!(&background.decisions[..], [RefreshDecision::RemoteReplaced { .. }]), "{background:?}");
    assert_eq!(w.head(), head);

    let over_https = w.account.refresh_with_token().unwrap();
    let over_ssh = ssh_refresh(&w, twin.path()).unwrap();
    assert!(matches!(over_https.state, SyncState::RemoteReplaced { .. }));
    assert_eq!(
        std::mem::discriminant(&over_https.state),
        std::mem::discriminant(&over_ssh.state)
    );
    assert_eq!(w.head(), head);
}

#[test]
fn local_changes_are_replayed_onto_a_background_fetch() {
    let w = world();
    w.set_up();
    let author = sync::Author { name: "iPhone".into(), email: "paw@localhost".into() };
    sync::commit(&w.repo(), "local/mine.gpg", b"mine", "Add given password for local/mine to store.", &author).unwrap();
    let remote_tip = w.remote.commit_file(NEW_ENTRY.into(), b"x".to_vec(), "Add.".into()).unwrap();

    let report = w.account.background_refresh(secs(25));
    assert_eq!(report.outcome, BackgroundOutcome::Updated);
    assert!(report.decisions.is_empty());
    let integration = report.integration.unwrap();
    assert_eq!((integration.fast_forwarded, integration.replayed), (1, 1));
    assert_eq!(report.unpushed, 1);
    let repo = git2::Repository::open(w.repo()).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_id(0).unwrap().to_string(), remote_tip);
    assert!(w.repo().join("local/mine.gpg").is_file() && w.repo().join(NEW_ENTRY).is_file());
    // The token never pushes.
    assert_eq!(w.remote.tip().unwrap(), remote_tip);
    w.assert_consistent();
}

// --- refusing to send the token where it does not belong ---------------------------------

#[test]
fn a_token_for_another_repository_is_not_used_for_this_store() {
    let w = world();
    w.github.set_extra_repositories(3);
    w.set_up();
    w.account.repositories().unwrap();
    w.account.select_repository("you/other-0001").unwrap();
    let before = w.git_requests();

    let error = w.account.refresh_with_token().unwrap_err();
    assert!(error.to_string().contains("GitHub sign-in is for you/other-0001"), "{error}");
    let report = w.account.background_refresh(secs(25));
    assert!(matches!(report.outcome, BackgroundOutcome::Failed { .. }), "{report:?}");
    assert_eq!(w.git_requests(), before);
}

#[test]
fn a_redirect_to_another_host_is_not_followed_and_carries_no_token() {
    let w = world();
    w.set_up();
    let elsewhere = SimBystander::start().unwrap();
    w.github.set_redirect_git(Some(elsewhere.url()));
    let head = w.head();

    let error = w.account.refresh_with_token().unwrap_err();
    assert!(error.to_string().contains("another address"), "{error}");
    assert_eq!(elsewhere.request_count(), 0);
    for request in elsewhere.requests() {
        for token in w.tokens() {
            assert!(!request.contains(&token));
        }
    }
    assert_eq!(w.head(), head);
}

#[test]
fn a_fetch_larger_than_the_cap_stops_and_changes_nothing() {
    let w = world();
    w.set_up();
    let head = w.head();
    for n in 0..20 {
        // Random, so it does not compress.
        let noise: Vec<u8> = (0..4096).map(|_| rand::random::<u8>()).collect();
        w.remote.commit_file(format!("noise/{n}.gpg"), noise, "Noise.".into()).unwrap();
    }
    w.account.set_download_limit(16 << 10);
    let error = w.account.refresh_with_token().unwrap_err();
    assert!(error.to_string().contains("paw stopped the download"), "{error}");
    assert_eq!(w.head(), head);
    w.assert_consistent();
}

// --- the background refresh: bounded and safe to kill ---------------------------------------

#[test]
fn stopping_the_background_refresh_at_any_step_leaves_the_store_consistent() {
    for step in [RefreshStep::Token, RefreshStep::Fetch, RefreshStep::Check, RefreshStep::Integrate] {
        let w = world();
        w.set_up();
        let head = w.head();
        let changed = w.remote.commit_file(NEW_ENTRY.into(), b"x".to_vec(), "Add.".into()).unwrap();

        let stopped = w.account.background_refresh_with(secs(25), &|at| at == step);
        assert_eq!(stopped.outcome, BackgroundOutcome::OutOfTime, "{step:?}");
        assert_eq!(w.head(), head, "{step:?}");
        w.assert_consistent();

        let again = w.account.background_refresh(secs(25));
        assert_eq!(again.outcome, BackgroundOutcome::Updated, "{step:?}");
        assert_eq!(w.head(), changed, "{step:?}");
        w.assert_consistent();
    }
}

#[test]
fn a_connection_dropped_mid_pack_changes_nothing_and_the_next_refresh_works() {
    let w = world();
    w.set_up();
    let head = w.head();
    let changed = w.remote.commit_file(NEW_ENTRY.into(), b"x".to_vec(), "Add.".into()).unwrap();
    w.github.set_drop_mid_pack(true);

    let report = w.account.background_refresh(secs(25));
    assert!(matches!(report.outcome, BackgroundOutcome::Offline { .. } | BackgroundOutcome::Failed { .. }), "{report:?}");
    assert_eq!(w.head(), head);
    w.assert_consistent();

    w.github.set_drop_mid_pack(false);
    assert_eq!(w.account.background_refresh(secs(25)).outcome, BackgroundOutcome::Updated);
    assert_eq!(w.head(), changed);
}

#[test]
fn a_background_refresh_with_no_time_left_sends_nothing() {
    let w = world();
    w.set_up();
    let before = w.github.request_count();
    assert_eq!(w.account.background_refresh(Duration::ZERO).outcome, BackgroundOutcome::OutOfTime);
    assert_eq!(w.github.request_count(), before);
}

#[test]
fn a_background_refresh_with_nothing_new_is_up_to_date() {
    let w = world();
    w.set_up();
    let report = w.account.background_refresh(secs(25));
    assert_eq!(report.outcome, BackgroundOutcome::UpToDate);
    assert!(report.decisions.is_empty());
}

#[test]
fn a_store_with_no_chosen_repository_is_not_refreshed() {
    let w = world();
    w.sign_in();
    let before = w.github.request_count();
    assert_eq!(w.account.background_refresh(secs(25)).outcome, BackgroundOutcome::NotLinked);
    assert_eq!(w.github.request_count(), before);
}

// --- the background task: stopped from another thread, bounded, busy ----------------

/// No lock is left: the store lock can be taken at once, and git left no
/// lock file behind.
fn assert_no_lock_left(w: &World) {
    let free = paw_core::lock::StoreLock::for_state_dir(w.state_dir()).exclusive_within(Duration::ZERO);
    assert!(free.is_ok(), "the store lock is still held");
    drop(free);
    let git = w.repo().join(".git");
    let left: Vec<PathBuf> =
        files_under(&git).into_iter().map(|(p, _)| p).filter(|p| p.extension().is_some_and(|e| e == "lock")).collect();
    assert!(left.is_empty(), "git lock files left: {left:?}");
}

fn wait_for_stalled_pack(w: &World) {
    let started = std::time::Instant::now();
    while w.github.stalled_packs() == 0 {
        assert!(started.elapsed() < secs(10), "the fetch never reached the pack");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_stop_from_another_thread_mid_transfer_ends_the_run_with_nothing_changed() {
    let w = Arc::new(world());
    w.set_up();
    let head = w.head();
    let changed = w.remote.commit_file(NEW_ENTRY.into(), vec![b's'; 64 << 10], "A big one.".into()).unwrap();
    w.github.stall_packs();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let running = {
        let (w, stop) = (w.clone(), stop.clone());
        std::thread::spawn(move || w.account.background_refresh_with(secs(25), &|_| stop.load(Ordering::SeqCst)))
    };
    wait_for_stalled_pack(&w);

    // The expiration handler: stop, then the next chunk ends the transfer.
    stop.store(true, Ordering::SeqCst);
    w.github.release_packs();
    let report = running.join().unwrap();

    assert_eq!(report.outcome, BackgroundOutcome::OutOfTime, "{report:?}");
    assert_eq!(w.head(), head);
    w.assert_consistent();
    assert_no_lock_left(&w);
    assert_eq!(w.account.background_refresh(secs(25)).outcome, BackgroundOutcome::Updated);
    assert_eq!(w.head(), changed);
}

#[test]
fn a_stalled_transfer_ends_by_the_deadline() {
    let w = world();
    w.set_up();
    let head = w.head();
    w.remote.commit_file(NEW_ENTRY.into(), vec![b's'; 64 << 10], "A big one.".into()).unwrap();
    w.github.stall_packs();

    let started = std::time::Instant::now();
    let report = w.account.background_refresh(secs(2));
    let took = started.elapsed();
    w.github.release_packs();

    assert_eq!(report.outcome, BackgroundOutcome::OutOfTime, "{report:?}");
    assert!(took < secs(6), "the run took {took:?} with a 2 s budget");
    assert_eq!(w.head(), head);
    w.assert_consistent();
    assert_no_lock_left(&w);
}

#[test]
fn a_store_in_use_ends_the_background_run_busy_and_changes_nothing() {
    let w = world();
    w.set_up();
    let head = w.head();
    let changed = w.remote.commit_file(NEW_ENTRY.into(), b"x".to_vec(), "Add.".into()).unwrap();
    // A save in the app (another thread) holds the store.
    let (taken, release) = (std::sync::mpsc::channel(), std::sync::mpsc::channel::<()>());
    let holder = {
        let state = w.state_dir().to_path_buf();
        let (taken, release) = (taken.0, release.1);
        std::thread::spawn(move || {
            let held = paw_core::lock::StoreLock::for_state_dir(&state).exclusive().unwrap();
            taken.send(()).unwrap();
            release.recv().unwrap();
            drop(held);
        })
    };
    taken.1.recv().unwrap();

    let started = std::time::Instant::now();
    let report = w.account.background_refresh(secs(25));
    let took = started.elapsed();
    release.0.send(()).unwrap();
    holder.join().unwrap();

    assert_eq!(report.outcome, BackgroundOutcome::Busy, "{report:?}");
    assert!(took < secs(10), "waited {took:?} for the store");
    assert_eq!(w.head(), head);
    w.assert_consistent();
    assert_eq!(w.account.background_refresh(secs(25)).outcome, BackgroundOutcome::Updated);
    assert_eq!(w.head(), changed);
}

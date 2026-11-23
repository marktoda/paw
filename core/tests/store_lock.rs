//! One lock per store (`paw_core::lock`): threads and processes that use one
//! store at once queue for its critical sections, never hold it across a
//! transfer or a wait for the key, and leave a consistent store whatever
//! order they ran in. Against the simulated GitHub in front of the
//! simulated SSH host, with a store set up the way the app sets it up.
//!
//! The cross-process tests run this test binary again as a helper
//! (`child_process_helper`, which does nothing unless asked).

#![cfg(feature = "sim")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use paw_core::ffi::sync::StoreSync;
use paw_core::github::{Endpoints, GitHubAccount, RepositoryList, SignInState, TokenVault, VaultError};
use paw_core::lock::{STORE_LOCK_FILE, StoreLock};
use paw_core::setup::touch::TouchExpectation;
use paw_core::setup::{SetupEvents, SetupFlow, SetupState};
use paw_core::sim::github::{SimGitHub, SimPollClock};
use paw_core::sim::remote::SimRemote;
use paw_core::sim::{SimTouch, SimulatedKey};
use paw_core::ssh::CardSigner;
use paw_core::store::PassStore;
use paw_core::sync::{self, operation, queue};
use paw_core::write::{EntryDraft, WriteError, WriteStore};

const PIN: &str = "123456";
const REPOSITORY: &str = "you/password-store";
const CHILD: &str = "PAW_LOCK_TEST_CHILD";

// --- a store set up as the app sets it up ---------------------------------------------

/// Tokens kept in a file, so two processes share them as the app and its
/// extension share the keychain.
struct FileVault(PathBuf);

impl FileVault {
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl TokenVault for FileVault {
    fn store(&self, name: String, secret: Vec<u8>) -> Result<(), VaultError> {
        let failed = |e: std::io::Error| VaultError::Failed { message: e.to_string() };
        let temporary = self.0.join(format!("{name}.{}.{:?}.tmp", std::process::id(), std::thread::current().id()));
        std::fs::write(&temporary, secret).map_err(failed)?;
        std::fs::rename(&temporary, self.path(&name)).map_err(failed)
    }

    fn load(&self, name: String) -> Result<Option<Vec<u8>>, VaultError> {
        match std::fs::read(self.path(&name)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(VaultError::Failed { message: e.to_string() }),
        }
    }

    fn remove(&self, name: String) -> Result<(), VaultError> {
        let _ = std::fs::remove_file(self.path(&name));
        Ok(())
    }
}

struct World {
    key: Arc<SimulatedKey>,
    remote: Arc<SimRemote>,
    github: Arc<SimGitHub>,
    /// The wall clock, in a file a child process reads too.
    wall: PathBuf,
    /// Holds the state directory (`state/`), the vault (`vault/`) and the
    /// wall clock.
    dir: tempfile::TempDir,
    state: PathBuf,
    account: Arc<GitHubAccount>,
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn read_wall(path: &Path) -> u64 {
    std::fs::read_to_string(path).ok().and_then(|t| t.trim().parse().ok()).unwrap_or_else(unix_now)
}

fn account(endpoints: Endpoints, top: &Path) -> GitHubAccount {
    let vault: Arc<dyn TokenVault> = Arc::new(FileVault(top.join("vault")));
    let wall = top.join("wall");
    GitHubAccount::with(endpoints, &top.join("state"), vault, SimPollClock::new(), Arc::new(move || read_wall(&wall)))
}

struct Quiet;

impl SetupEvents for Quiet {
    fn touch_needed(&self, _expectation: TouchExpectation) {}

    fn progress(&self, _received: u32, _total: u32) -> bool {
        true
    }
}

/// Signed in to the simulated GitHub, the store picked, cloned over SSH
/// with the key, and the key check done: entries can be saved, sent and
/// refreshed.
fn world_with(key: Arc<SimulatedKey>) -> World {
    let remote = SimRemote::start(key.clone()).unwrap();
    let github = SimGitHub::start_for(&remote, REPOSITORY).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("vault")).unwrap();
    std::fs::create_dir_all(dir.path().join("state")).unwrap();
    let wall = dir.path().join("wall");
    std::fs::write(&wall, unix_now().to_string()).unwrap();
    let account = Arc::new(account(github.endpoints(), dir.path()));
    let state = dir.path().join("state");
    let w = World { key, remote, github, wall, dir, state, account };

    let sign_in = w.account.start_sign_in().unwrap();
    while sign_in.poll().unwrap() != SignInState::SignedIn {}
    let RepositoryList::Repositories { .. } = w.account.repositories().unwrap() else {
        panic!("not installed");
    };
    let address = w.account.select_repository(REPOSITORY).unwrap();
    let mut flow = SetupFlow::open(w.state()).unwrap();
    flow.read_key(w.key.clone()).unwrap();
    flow.set_address(&address).unwrap();
    let SetupState::ConfirmHost { fingerprint, .. } = flow.check_host().unwrap() else {
        panic!("expected ConfirmHost");
    };
    flow.confirm_host(&fingerprint).unwrap();
    let state = flow.download(w.key.clone(), PIN, Arc::new(Quiet)).unwrap();
    assert!(matches!(state, SetupState::TestDecrypt { .. }), "{state:?}");
    let state = flow.test_decrypt(w.key.clone(), PIN, Arc::new(Quiet)).unwrap();
    assert!(matches!(state, SetupState::Done { .. }), "{state:?}");
    w
}

fn world() -> World {
    world_with(SimulatedKey::new(PIN.into(), false, 1000))
}

impl World {
    fn state(&self) -> &Path {
        &self.state
    }

    fn repo(&self) -> PathBuf {
        self.state().join("repo")
    }

    fn writer(&self) -> WriteStore {
        WriteStore::open(self.state())
    }

    /// Moves the wall clock on past the access token's life.
    fn expire_token(&self) {
        let now = read_wall(&self.wall);
        std::fs::write(&self.wall, (now + 9 * 3600).to_string()).unwrap();
    }

    fn tokens(&self) -> usize {
        self.github.all_tokens().len()
    }

    fn save(&self, name: &str) -> Result<paw_core::write::Saved, WriteError> {
        save_in(&self.writer(), name)
    }

    fn send(&self) -> Result<operation::SyncReport, sync::SyncError> {
        let signer = CardSigner::new(self.key.clone(), PIN, || {}).unwrap();
        operation::send_checked(&self.remote.remote().unwrap(), &signer, &self.repo(), self.state())
    }

    /// The working copy matches the branch, nothing is half done, and no
    /// git lock file is left.
    fn assert_consistent(&self) {
        let repo = git2::Repository::open(self.repo()).unwrap();
        let mut options = git2::StatusOptions::new();
        options.include_untracked(true);
        let statuses = repo.statuses(Some(&mut options)).unwrap();
        let dirty: Vec<String> = statuses.iter().filter_map(|s| s.path().ok().map(str::to_owned)).collect();
        assert!(dirty.is_empty(), "working copy differs from the branch: {dirty:?}");
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        let left = git_lock_files(&self.repo().join(".git"));
        assert!(left.is_empty(), "git lock files left: {left:?}");
    }

    /// Whether the store's branch (local) or the remote's has `entry`.
    fn has(&self, repo: &git2::Repository, branch: &str, entry: &str) -> bool {
        let tree = repo.find_reference(branch).unwrap().peel_to_tree().unwrap();
        tree.get_path(Path::new(&format!("{entry}.gpg"))).is_ok()
    }

    fn local_has(&self, entry: &str) -> bool {
        let repo = git2::Repository::open(self.repo()).unwrap();
        self.has(&repo, "HEAD", entry) && self.repo().join(format!("{entry}.gpg")).is_file()
    }

    fn remote_has(&self, entry: &str) -> bool {
        let repo = git2::Repository::open_bare(self.remote.bare()).unwrap();
        self.has(&repo, "refs/heads/main", entry)
    }
}

fn save_in(writer: &WriteStore, name: &str) -> Result<paw_core::write::Saved, WriteError> {
    let mut draft = EntryDraft::new("lock");
    draft.set_name(name);
    draft.set_password(&format!("pw-{name}"));
    writer.save(&draft).map(|(saved, _)| saved)
}

fn git_lock_files(git: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![git.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for child in std::fs::read_dir(&folder).unwrap().flatten() {
            let path = child.path();
            if path.starts_with(git.join("objects")) || path.starts_with(git.join("paw")) {
                continue;
            }
            if child.file_type().unwrap().is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|e| e == "lock") {
                found.push(path);
            }
        }
    }
    found
}

/// Runs `work` on another thread and fails the test if it has not finished
/// within `limit`: a deadlock shows as a failure, not a hung test run.
fn within<T: Send + 'static>(limit: Duration, what: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(work());
    });
    match finished.recv_timeout(limit) {
        Ok(value) => value,
        Err(_) => panic!("{what} did not finish within {limit:?}: deadlocked or stuck behind a held lock"),
    }
}

// --- the race that failed on GitHub's runner -----------------------------------------------

#[test]
fn concurrent_token_refreshes_never_collide_on_a_ref_200_rounds() {
    let w = Arc::new(world());
    let threads = 4;
    for round in 0..200 {
        w.expire_token();
        if round % 5 == 0 {
            // Something to integrate, so every round also moves refs.
            w.remote.commit_file(format!("remote/r{round}.gpg"), vec![b'x'; 8], format!("Round {round}.")).unwrap();
        }
        let before = w.tokens();
        let start = Arc::new(Barrier::new(threads));
        let running: Vec<_> = (0..threads)
            .map(|_| {
                let (w, start) = (w.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    w.account.refresh_with_token()
                })
            })
            .collect();
        for thread in running {
            if let Err(error) = thread.join().unwrap() {
                panic!("round {round}: {error}");
            }
        }
        assert_eq!(w.tokens(), before + 2, "round {round}: exactly one rotation");
    }
    w.assert_consistent();
    assert!(w.local_has("remote/r195"));
}

// --- many operations at once ---------------------------------------------------------------

#[test]
fn refresh_save_undo_read_and_send_at_once_lose_nothing() {
    let w = Arc::new(world());
    let stop = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();

    // Two savers: every save either lands or is reported failed.
    let saves: Arc<Mutex<BTreeMap<String, bool>>> = Arc::default();
    for saver in 0..2 {
        let (w, saves) = (w.clone(), saves.clone());
        workers.push(std::thread::spawn(move || {
            for n in 0..15 {
                let name = format!("s{saver}-{n}");
                let ok = w.save(&name).is_ok();
                saves.lock().unwrap().insert(name, ok);
            }
        }));
    }
    // An undoer: saves its own entry, then takes that change back. If a
    // send pushed it first, taking it back is refused and it stays.
    let undone: Arc<Mutex<BTreeMap<String, bool>>> = Arc::default();
    {
        let (w, undone) = (w.clone(), undone.clone());
        workers.push(std::thread::spawn(move || {
            for n in 0..10 {
                let name = format!("u-{n}");
                let Ok(saved) = w.save(&name) else { continue };
                let taken_back = queue::discard(&w.repo(), &saved.commits[0]).is_ok();
                undone.lock().unwrap().insert(name, taken_back);
            }
        }));
    }
    // Readers: a listing and every entry in it, all of one moment.
    let reads = Arc::new(AtomicU64::new(0));
    for _ in 0..2 {
        let (w, stop, reads) = (w.clone(), stop.clone(), reads.clone());
        workers.push(std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let _reading = StoreLock::for_state_dir(w.state()).shared().unwrap();
                let store = PassStore::open(w.repo().to_string_lossy().into_owned()).unwrap();
                for entry in store.entries() {
                    let bytes = store.read_entry(&entry.path).unwrap();
                    assert!(!bytes.is_empty(), "{} read empty", entry.path);
                }
                reads.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    // Background refreshes with the token, with the token running out now
    // and then (a refresh rotates it, and ends the other's fetch in flight,
    // which then uses the new one).
    let refreshes = Arc::new(AtomicU64::new(0));
    for refresher in 0..2 {
        let (w, stop, refreshes) = (w.clone(), stop.clone(), refreshes.clone());
        workers.push(std::thread::spawn(move || {
            let mut n = 0;
            while !stop.load(Ordering::SeqCst) {
                if refresher == 0 && n % 4 == 0 {
                    w.expire_token();
                }
                w.account.refresh_with_token().unwrap();
                refreshes.fetch_add(1, Ordering::SeqCst);
                n += 1;
            }
        }));
    }
    // The desktop keeps pushing, so integrate has to replay.
    {
        let (w, stop) = (w.clone(), stop.clone());
        workers.push(std::thread::spawn(move || {
            let mut n = 0;
            while !stop.load(Ordering::SeqCst) && n < 20 {
                w.remote.commit_file(format!("desktop/d{n}.gpg"), vec![b'd'; 8], format!("Desktop {n}.")).unwrap();
                n += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
        }));
    }
    // One sender with the key (the app has one key and talks to it from
    // one thread).
    let sends = Arc::new(AtomicU64::new(0));
    {
        let (w, stop, sends) = (w.clone(), stop.clone(), sends.clone());
        workers.push(std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match w.send() {
                    Ok(_) | Err(sync::SyncError::RemoteKeptMoving) => {}
                    Err(other) => panic!("send failed: {other}"),
                }
                sends.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }

    // The savers and the undoer finish; then everything else stops.
    let finite = 3;
    let mut workers = workers.into_iter();
    for worker in workers.by_ref().take(finite) {
        worker.join().unwrap();
    }
    std::thread::sleep(Duration::from_millis(300));
    stop.store(true, Ordering::SeqCst);
    for worker in workers {
        worker.join().unwrap();
    }
    // Everything still waiting goes out once all is quiet.
    let report = w.send().unwrap();
    assert_eq!(report.state, operation::SyncState::UpToDate);

    w.assert_consistent();
    let saves = saves.lock().unwrap();
    assert_eq!(saves.len(), 30);
    for (name, ok) in saves.iter() {
        let entry = format!("lock/{name}");
        assert_eq!(w.local_has(&entry), *ok, "{entry}: saved {ok}, but here {}", w.local_has(&entry));
        assert_eq!(w.remote_has(&entry), *ok, "{entry}: saved {ok}, but sent {}", w.remote_has(&entry));
    }
    assert!(saves.values().all(|ok| *ok), "no save failed: {saves:?}");
    for (name, taken_back) in undone.lock().unwrap().iter() {
        let entry = format!("lock/{name}");
        assert_eq!(w.local_has(&entry), !taken_back, "{entry}");
        assert_eq!(w.remote_has(&entry), !taken_back, "{entry}");
    }
    for n in 0..20 {
        assert!(w.local_has(&format!("desktop/d{n}")), "desktop/d{n} was lost");
    }
    assert!(queue::unpushed(&w.repo()).unwrap().is_empty());
    let counts = (reads.load(Ordering::SeqCst), refreshes.load(Ordering::SeqCst), sends.load(Ordering::SeqCst));
    eprintln!("reads, refreshes, sends: {counts:?}");
    assert!(counts.0 > 0 && counts.1 > 0 && counts.2 > 0, "{counts:?}");
}

// --- readers -------------------------------------------------------------------------------

#[test]
fn a_reader_never_sees_a_checkout_halfway() {
    let w = Arc::new(world());
    // Two generations of 60 entries: each integrate rewrites all of them.
    let generation = |g: u8| -> Vec<(String, Vec<u8>)> {
        (0..60).map(|i| (format!("bulk/e{i:02}.gpg"), vec![b'A' + g; 4096])).collect()
    };
    let push_generation = |g: u8| {
        let repo = git2::Repository::open_bare(w.remote.bare()).unwrap();
        let parent = repo.find_commit(repo.refname_to_id("refs/heads/main").unwrap()).unwrap();
        let mut index = git2::Index::new().unwrap();
        index.read_tree(&parent.tree().unwrap()).unwrap();
        for (path, bytes) in generation(g) {
            let blob = repo.blob(&bytes).unwrap();
            let entry = git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: 0o100644,
                uid: 0,
                gid: 0,
                file_size: bytes.len() as u32,
                id: blob,
                flags: path.len() as u16,
                flags_extended: 0,
                path: path.into_bytes(),
            };
            index.add(&entry).unwrap();
        }
        let tree = repo.find_tree(index.write_tree_to(&repo).unwrap()).unwrap();
        let who = git2::Signature::now("Desktop", "desktop@example.com").unwrap();
        repo.commit(Some("refs/heads/main"), &who, &who, &format!("Generation {g}."), &tree, &[&parent]).unwrap();
    };
    push_generation(0);
    w.account.refresh_with_token().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let snapshots = Arc::new(AtomicU64::new(0));
    let readers: Vec<_> = (0..3)
        .map(|_| {
            let (w, stop, snapshots) = (w.clone(), stop.clone(), snapshots.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    // One reading moment: the listing and every file.
                    let _reading = StoreLock::for_state_dir(w.state()).shared().unwrap();
                    let store = PassStore::open(w.repo().to_string_lossy().into_owned()).unwrap();
                    let mut seen = Vec::new();
                    for entry in store.entries().iter().filter(|e| e.path.starts_with("bulk/")) {
                        let bytes = store.read_entry(&entry.path).unwrap();
                        assert_eq!(bytes.len(), 4096, "{}: a file written halfway", entry.path);
                        assert!(bytes.iter().all(|b| *b == bytes[0]), "{}: mixed contents", entry.path);
                        seen.push(bytes[0]);
                    }
                    assert_eq!(seen.len(), 60, "a listing halfway through a checkout");
                    assert!(seen.iter().all(|g| *g == seen[0]), "two generations at once: {seen:?}");
                    snapshots.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    // And a reader without an outer lock: each file whole.
    let lone = {
        let (w, stop) = (w.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let store = PassStore::open(w.repo().to_string_lossy().into_owned()).unwrap();
                for i in 0..60 {
                    if let Ok(bytes) = store.read_entry(&format!("bulk/e{i:02}")) {
                        assert_eq!(bytes.len(), 4096, "a file written halfway");
                    }
                }
            }
        })
    };
    for g in 1..=12 {
        push_generation(g);
        w.account.refresh_with_token().unwrap();
    }
    stop.store(true, Ordering::SeqCst);
    for reader in readers {
        reader.join().unwrap();
    }
    lone.join().unwrap();
    w.assert_consistent();
    assert!(snapshots.load(Ordering::SeqCst) > 0);
}

// --- nothing slow under the lock ---------------------------------------------------------------

#[test]
fn a_save_and_a_read_finish_while_a_fetch_is_stalled_mid_transfer() {
    let w = Arc::new(world());
    w.remote.commit_file("remote/stalled.gpg".into(), vec![b's'; 64 << 10], "A big one.".into()).unwrap();
    w.github.stall_packs();
    let refreshing = {
        let w = w.clone();
        std::thread::spawn(move || w.account.refresh_with_token())
    };
    let started = Instant::now();
    while w.github.stalled_packs() == 0 {
        assert!(started.elapsed() < Duration::from_secs(10), "the fetch never reached the pack");
        std::thread::sleep(Duration::from_millis(5));
    }

    // The fetch is halfway through its pack. A save and a listing go ahead.
    let saving = Instant::now();
    let w2 = w.clone();
    within(Duration::from_secs(5), "a save during a stalled fetch", move || w2.save("during-fetch").unwrap());
    let w2 = w.clone();
    let listed = within(Duration::from_secs(5), "a listing during a stalled fetch", move || {
        PassStore::open(w2.repo().to_string_lossy().into_owned()).unwrap().entry_count()
    });
    assert!(saving.elapsed() < Duration::from_secs(2), "the save waited {:?}", saving.elapsed());
    assert!(listed > 0);
    // Taking the store lock outright works too: the fetch does not hold it.
    let held = StoreLock::for_state_dir(w.state()).exclusive_within(Duration::from_millis(200));
    assert!(held.is_ok(), "the store lock is free during the transfer");
    drop(held);
    assert_eq!(w.github.stalled_packs(), 1, "still stalled");

    w.github.release_packs();
    let report = refreshing.join().unwrap().unwrap();
    assert!(report.integration.fast_forwarded > 0 || report.integration.replayed > 0, "{report:?}");
    assert!(w.local_has("remote/stalled"));
    assert!(w.local_has("lock/during-fetch"), "the save made during the fetch was kept by the integrate");
    w.assert_consistent();
}

#[test]
fn a_save_finishes_while_a_send_waits_for_a_touch() {
    let key = SimulatedKey::new(PIN.into(), false, 20_000);
    let w = Arc::new(world_with(key.clone()));
    w.save("before-touch").unwrap();
    key.set_authentication_touch(SimTouch::EachUse);
    let sync = StoreSync::open(w.state().to_string_lossy().into_owned()).unwrap();
    let sending = {
        let (sync, key) = (sync.clone(), key.clone());
        std::thread::spawn(move || sync.send(key.as_connection(), PIN.into()))
    };
    let started = Instant::now();
    while !key.waiting_for_touch() {
        assert!(started.elapsed() < Duration::from_secs(10), "the send never asked for a touch");
        std::thread::sleep(Duration::from_millis(5));
    }

    // The key is waiting for a finger. Saves, reads and the queue go ahead.
    let w2 = w.clone();
    within(Duration::from_secs(5), "a save during a touch wait", move || w2.save("during-touch").unwrap());
    let w2 = w.clone();
    let queued = within(Duration::from_secs(5), "the queue during a touch wait", move || {
        queue::unpushed(&w2.repo()).unwrap().len()
    });
    assert_eq!(queued, 2);
    assert!(key.waiting_for_touch(), "still waiting");

    key.touch();
    let report = sending.join().unwrap().unwrap();
    // The send may or may not include the save made while it waited; what
    // it did not send is still waiting, never lost.
    assert!(report.pushed >= 1, "{report:?}");
    let left = queue::unpushed(&w.repo()).unwrap().len() as u32;
    assert_eq!(report.pushed + left, 2);
    key.set_authentication_touch(SimTouch::Off);
    w.send().unwrap();
    assert!(w.remote_has("lock/before-touch") && w.remote_has("lock/during-touch"));
    w.assert_consistent();
}

// --- stale git locks ---------------------------------------------------------------------------

#[test]
fn git_lock_files_left_by_a_killed_process_are_removed_under_the_lock() {
    let w = world();
    let git = w.repo().join(".git");
    let stale = [
        git.join("index.lock"),
        git.join("HEAD.lock"),
        git.join("refs/heads/main.lock"),
        git.join("refs/paw/last-seen/main.lock"),
        git.join("refs/remotes/origin/main.lock"),
        git.join("logs/refs/heads/main.lock"),
    ];
    for path in &stale {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }
    // Without the cleanup these fail every later change ("failed to create
    // locked file ...: File exists").
    w.save("after-a-crash").unwrap();
    w.remote.commit_file("remote/after.gpg".into(), b"x".to_vec(), "After.".into()).unwrap();
    w.account.refresh_with_token().unwrap();
    for path in &stale {
        assert!(!path.exists(), "{} is still there", path.display());
    }
    assert!(w.local_has("lock/after-a-crash") && w.local_has("remote/after"));
    w.assert_consistent();
    // paw's own lock file is not git's and stays.
    assert!(w.state().join(STORE_LOCK_FILE).exists());
}

// --- other processes -----------------------------------------------------------------------------

/// The helper the cross-process tests start as a child process. Does
/// nothing in an ordinary test run.
#[test]
fn child_process_helper() {
    let Ok(role) = std::env::var(CHILD) else {
        return;
    };
    let top = PathBuf::from(std::env::var("PAW_LOCK_TEST_DIR").unwrap());
    let state = top.join("state");
    match role.as_str() {
        // Takes the store lock, leaves git lock files as a write cut short
        // would, says so, and waits to be killed.
        "hold" => {
            let _held = StoreLock::for_state_dir(&state).exclusive().unwrap();
            let git = state.join("repo/.git");
            std::fs::write(git.join("index.lock"), b"").unwrap();
            std::fs::write(git.join("refs/heads/main.lock"), b"").unwrap();
            std::fs::write(top.join("child-holds"), b"").unwrap();
            std::thread::sleep(Duration::from_secs(120));
        }
        // Refreshes with the token from several threads at once, when told
        // to, as the extension and the app might.
        "refresh" => {
            let endpoints = Endpoints::simulated(
                &std::env::var("PAW_LOCK_TEST_URL").unwrap(),
                &std::env::var("PAW_LOCK_TEST_URL").unwrap(),
                &std::env::var("PAW_LOCK_TEST_CLIENT").unwrap(),
                Some((REPOSITORY.into(), std::env::var("PAW_LOCK_TEST_SSH").unwrap())),
            );
            let account = Arc::new(account(endpoints, &top));
            let rounds: u32 = std::env::var("PAW_LOCK_TEST_ROUNDS").unwrap().parse().unwrap();
            for round in 0..rounds {
                let go = top.join(format!("go-{round}"));
                let started = Instant::now();
                while !go.exists() {
                    assert!(started.elapsed() < Duration::from_secs(60));
                    std::thread::sleep(Duration::from_millis(1));
                }
                let threads: Vec<_> = (0..3)
                    .map(|_| {
                        let account = account.clone();
                        std::thread::spawn(move || account.refresh_with_token().map(drop).map_err(|e| e.to_string()))
                    })
                    .collect();
                let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
                let failed: Vec<_> = results.into_iter().filter_map(Result::err).collect();
                std::fs::write(top.join(format!("done-{round}")), failed.join("\n")).unwrap();
            }
        }
        other => panic!("unknown child role {other}"),
    }
}

fn child(role: &str, top: &Path, env: &[(&str, String)]) -> Child {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "child_process_helper", "--nocapture", "--test-threads=1"])
        .env(CHILD, role)
        .env("PAW_LOCK_TEST_DIR", top)
        .stdout(std::process::Stdio::null());
    for (name, value) in env {
        command.env(name, value);
    }
    command.spawn().unwrap()
}

fn wait_for(path: &Path) {
    let started = Instant::now();
    while !path.exists() {
        assert!(started.elapsed() < Duration::from_secs(30), "{} never appeared", path.display());
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn another_process_holding_the_lock_is_waited_for_and_its_death_frees_it() {
    let w = world();
    let mut holder = child("hold", w.dir.path(), &[]);
    wait_for(&w.dir.path().join("child-holds"));

    // Another process has it: a bounded wait, then a clear error.
    let store = StoreLock::for_state_dir(w.state());
    let started = Instant::now();
    let refused = store.exclusive_within(Duration::from_millis(300)).unwrap_err();
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(refused.to_string().contains("busy"), "{refused}");
    assert!(store.shared_within(Duration::from_millis(100)).is_err(), "a reader waits for a writer in another process");

    // The holder is killed mid-write: the kernel drops its lock, and what
    // it left in `.git` does not stop the next change.
    holder.kill().unwrap();
    holder.wait().unwrap();
    let saved = within(Duration::from_secs(5), "a save after the holder died", {
        let writer = w.writer();
        move || save_in(&writer, "after-the-kill")
    });
    saved.unwrap();
    assert!(w.local_has("lock/after-the-kill"));
    w.assert_consistent();
}

#[test]
fn two_processes_refreshing_at_once_spend_the_refresh_token_once() {
    let w = world();
    let rounds = 10;
    let mut other = child(
        "refresh",
        w.dir.path(),
        &[
            ("PAW_LOCK_TEST_URL", w.github.url()),
            ("PAW_LOCK_TEST_CLIENT", w.github.client_id()),
            ("PAW_LOCK_TEST_SSH", w.remote.address()),
            ("PAW_LOCK_TEST_ROUNDS", rounds.to_string()),
        ],
    );
    let w = Arc::new(w);
    for round in 0..rounds {
        w.expire_token();
        w.remote.commit_file(format!("remote/p{round}.gpg"), b"x".to_vec(), format!("Round {round}.")).unwrap();
        let before = w.tokens();
        std::fs::write(w.dir.path().join(format!("go-{round}")), b"").unwrap();
        let threads: Vec<_> = (0..3)
            .map(|_| {
                let w = w.clone();
                std::thread::spawn(move || w.account.refresh_with_token())
            })
            .collect();
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        let done = w.dir.path().join(format!("done-{round}"));
        wait_for(&done);
        let failed = std::fs::read_to_string(&done).unwrap();
        assert!(failed.is_empty(), "round {round}: the other process failed: {failed}");
        assert_eq!(w.tokens(), before + 2, "round {round}: exactly one rotation between both processes");
    }
    assert!(other.wait().unwrap().success());
    w.assert_consistent();
    assert!(w.local_has(&format!("remote/p{}", rounds - 1)));
}

// --- no deadlock ------------------------------------------------------------------------------------

/// Every public entry point that changes a store, once on its own and once
/// from a thread that already holds the store's lock: each returns (with
/// its result or a clear error) instead of waiting on itself.
#[test]
fn every_mutating_entry_point_returns_even_when_its_thread_holds_the_lock() {
    let w = Arc::new(world());
    for nested in [false, true] {
        let w2 = w.clone();
        let what = if nested { "calls under the lock" } else { "plain calls" };
        let results = within(Duration::from_secs(120), what, move || {
            let w = w2;
            let _outer = nested.then(|| StoreLock::for_state_dir(w.state()).exclusive().unwrap());
            let tag = if nested { "n" } else { "p" };
            let repo = w.repo();
            let writer = w.writer();
            let author = sync::Author { name: "t".into(), email: "t@example.com".into() };
            let mut results: Vec<(&'static str, Result<(), String>)> = Vec::new();
            let mut note = |what, result: Result<(), String>| results.push((what, result));
            let text = |e: &dyn std::fmt::Display| e.to_string();

            note("save", save_in(&writer, &format!("{tag}-a")).map(drop).map_err(|e| text(&e)));
            note("rename", writer.rename(&format!("lock/{tag}-a"), &format!("lock/{tag}-b"), None).map(drop).map_err(|e| text(&e)));
            note("delete", writer.delete(&format!("lock/{tag}-b")).map(drop).map_err(|e| text(&e)));
            let saved = save_in(&writer, &format!("{tag}-c")).unwrap();
            note("discard", queue::discard(&repo, &saved.commits[0]).map(drop).map_err(|e| text(&e)));
            save_in(&writer, &format!("{tag}-d")).unwrap();
            note("undo_last", queue::undo_last(&repo).map(drop).map_err(|e| text(&e)));
            note("commit", sync::commit(&repo, &format!("lock/{tag}-e.gpg"), b"x", "Add.", &author).map(drop).map_err(|e| text(&e)));
            note("discard_all", queue::discard_all(&repo).map_err(|e| text(&e)));
            note("integrate", sync::integrate(&repo).map(drop).map_err(|e| text(&e)));
            note("accept_replaced_remote", sync::accept_replaced_remote(&repo, true).map(drop).map_err(|e| text(&e)));
            let hosts = paw_core::sync::hosts::KnownHosts::new(&w.state().join("known_hosts"));
            note("known_hosts.forget", hosts.forget("elsewhere.example", 22).map_err(|e| text(&e)));

            let ffi_writer = paw_core::ffi::write::StoreWriter::open(w.state().to_string_lossy().into_owned());
            note("accept_recipients", ffi_writer.accept_recipients(String::new()).map_err(|e| text(&e)));
            note("accept_all_recipients_at_setup", ffi_writer.accept_all_recipients_at_setup().map_err(|e| text(&e)));
            note("set_git_identity", ffi_writer.set_git_identity(None).map_err(|e| text(&e)));
            note("pin_card_key", ffi_writer.pin_card_key(w.key.clone().as_connection()).map(drop).map_err(|e| text(&e)));
            let sync_api = StoreSync::open(w.state().to_string_lossy().into_owned()).unwrap();
            note("store_sync.discard_all", sync_api.discard_all().map_err(|e| text(&e)));
            note("store_sync.undo_last", sync_api.undo_last().map(drop).map_err(|e| text(&e)));
            note("store_sync.accept_replaced_remote", sync_api.accept_replaced_remote(true).map(drop).map_err(|e| text(&e)));
            note("select_repository", w.account.select_repository(REPOSITORY).map(drop).map_err(|e| text(&e)));

            // These fetch. Under the lock they are refused at once (the
            // fetch lock is never taken under the store lock), never stuck.
            note("store_sync.refresh", sync_api.refresh(w.key.clone().as_connection(), PIN.into()).map(drop).map_err(|e| text(&e)));
            note("store_sync.send", sync_api.send(w.key.clone().as_connection(), PIN.into()).map(drop).map_err(|e| text(&e)));
            note("sync.push", {
                let signer = CardSigner::new(w.key.clone(), PIN, || {}).unwrap();
                let remote = w.remote.remote().unwrap();
                sync::Connection::open(&remote, &signer).map_err(|e| text(&e)).and_then(|c| sync::push(&c, &repo).map_err(|e| text(&e)))
            });
            note("refresh_with_token", w.account.refresh_with_token().map(drop).map_err(|e| text(&e)));
            let background = w.account.background_refresh(Duration::from_secs(25));
            note("background_refresh", match background.outcome {
                paw_core::github::BackgroundOutcome::Failed { message } => Err(message),
                _ => Ok(()),
            });
            results
        });
        let fetching = ["store_sync.refresh", "store_sync.send", "sync.push", "refresh_with_token", "background_refresh"];
        for (what, result) in results {
            match result {
                Ok(()) => assert!(!(nested && fetching.contains(&what)), "{what} fetched under the lock"),
                Err(message) if nested && fetching.contains(&what) => {
                    assert!(message.contains("while holding the store"), "{what}: {message}");
                }
                Err(message) => panic!("{what} ({}) failed: {message}", if nested { "nested" } else { "plain" }),
            }
        }
    }
    // Afterwards the store works as before.
    w.save("afterwards").unwrap();
    w.send().unwrap();
    w.assert_consistent();
}

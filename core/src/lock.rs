//! One lock per store, between threads and between processes.
//!
//! The app and its AutoFill extension share a store through the App Group
//! container, and inside the app a background refresh, a foreground sync,
//! a save and the list can all run at once on different threads. Without a
//! lock two of them can write one ref at the same moment (libgit2 then
//! fails with "failed to create locked file ...: File exists"), or a reader
//! can list a working copy that a checkout is halfway through.
//!
//! [`StoreLock`] names a store's lock. It has two layers:
//!
//! - **In the process**, a readers-writer gate per store, keyed by the
//!   store's canonical directory, so threads queue (a waiting writer goes
//!   before new readers).
//! - **Between processes**, an advisory `flock` on a lock file
//!   ([`STORE_LOCK_FILE`] in the state directory), taken with the standard
//!   library's `File::try_lock`/`try_lock_shared` (Rust 1.89): no new crate
//!   and no C. The kernel drops it when its holder dies, so a killed
//!   extension never leaves the store locked.
//!
//! Every wait is bounded ([`DEFAULT_WAIT`] to change, [`SHARED_WAIT`] to
//! read); past it the operation fails with [`busy`], whose message starts
//! with [`STORE_BUSY_PREFIX`] ([`is_store_busy`]), and changes nothing.
//!
//! **The only file lock is the store lock**, and every holder keeps it for
//! local file work only: never across a transfer, a wait for the key or a
//! callback into the app. A process suspended while holding a file lock in
//! the App Group container is killed by iOS (`0xdead10cc`), so nothing
//! slow may be under it. The fetch and push locks are in-process only:
//! only the app fetches and sends (the extension must never do either).
//!
//! **Which lock.** A store set up by paw has its repository at
//! `<state>/repo`, so its lock is the state directory's. A repository
//! anywhere else (tests, tools) is a store of its own, locked in
//! `<repo>/.git/paw/`.
//!
//! **Evidence, not re-entry.** Code that changes a store takes an
//! [`Exclusive`] as a parameter, so it cannot be called without the lock
//! held; public entry points take the lock and pass it down. As a safety
//! net a thread that already holds a store's lock and asks for it again
//! gets a nested guard instead of waiting on itself (asking to change a
//! store it is only reading is refused with an error), so no public
//! function can deadlock on its own thread.
//!
//! **Stale git locks.** paw writes git's own files only with the exclusive
//! lock held, so a `*.lock` file that git left in `.git` (`index.lock`,
//! `refs/.../main.lock`) when a holder was killed can only be dead: taking
//! the exclusive lock removes them ([`remove_stale_git_locks`]). Fetches
//! write objects, their private refs and `shallow.lock` without the store
//! lock, under the store's fetch lock ([`StoreLock::fetching`]), which
//! cleans those itself.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::PawError;

/// The store lock's file, in the state directory: paw's only file lock.
pub const STORE_LOCK_FILE: &str = crate::layout::STORE_LOCK;
/// The longest a change waits for the store lock. Every holder keeps it
/// for local file work only, so this is far longer than any real wait.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(15);
/// The longest a read waits: reads are cheap to wait for, and a failed
/// listing is worse than a slow one.
pub const SHARED_WAIT: Duration = Duration::from_secs(30);
/// The longest recording a push that has landed waits for the lock.
pub const RECORD_WAIT: Duration = Duration::from_secs(120);
/// The longest a fetch waits for another fetch of the same store to finish.
pub const FETCH_WAIT: Duration = Duration::from_secs(60);
/// Where a repository outside a state directory keeps its lock files.
const GIT_LOCK_DIR: &str = "paw";

/// How the message of [`busy`] starts, whichever error type carries it.
/// Stable: the app recognises it ([`is_store_busy`]).
pub const STORE_BUSY_PREFIX: &str = "the password store is busy";

/// The error a lock that could not be had in time becomes. Nothing was
/// changed.
pub fn busy() -> PawError {
    PawError::Store(format!(
        "{STORE_BUSY_PREFIX}: another part of paw (or the AutoFill extension) is using it; try again"
    ))
}

/// Whether an error's message (any of paw's error types) says the store
/// was busy: nothing was changed and trying again will work. Not a broken
/// store.
pub fn is_store_busy(message: &str) -> bool {
    message.contains(STORE_BUSY_PREFIX)
}

#[cfg(feature = "sim")]
thread_local! {
    /// [`RECORD_WAIT`], unless a test lowered it for this thread.
    static RECORD_WAIT_HERE: std::cell::Cell<Duration> = const { std::cell::Cell::new(RECORD_WAIT) };
}

/// How long this thread waits to record a push that landed.
#[cfg(feature = "sim")]
pub(crate) fn record_wait() -> Duration {
    RECORD_WAIT_HERE.with(std::cell::Cell::get)
}

/// How long this thread waits to record a push that landed.
#[cfg(not(feature = "sim"))]
pub(crate) fn record_wait() -> Duration {
    RECORD_WAIT
}

/// Runs `work` with this thread's wait to record a push lowered to `wait`.
/// For tests: `sim` builds only.
#[cfg(feature = "sim")]
pub fn with_record_wait<T>(wait: Duration, work: impl FnOnce() -> T) -> T {
    let before = RECORD_WAIT_HERE.with(|w| w.replace(wait));
    let result = work();
    RECORD_WAIT_HERE.with(|w| w.set(before));
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Shared,
    Exclusive,
}

/// One lock: its in-process key and, if it has one, its lock file.
#[derive(Clone, Debug)]
struct Lock {
    key: PathBuf,
    file: Option<PathBuf>,
}

/// A store's locks. Cheap to make; nothing is taken until asked.
#[derive(Clone, Debug)]
pub struct StoreLock {
    /// The canonical directory that names the store.
    root: PathBuf,
    /// Where the lock files go; `None` for a folder that is not a git
    /// repository (nothing paw writes, so the in-process gate is enough).
    files: Option<PathBuf>,
    /// The store's repository, for stale git locks.
    repo: PathBuf,
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl StoreLock {
    /// The lock of the store whose state directory is `state_dir` (its
    /// repository at `<state_dir>/repo`). The directory is created if need
    /// be.
    pub fn for_state_dir(state_dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(state_dir);
        let root = canonical(state_dir);
        Self { files: Some(root.clone()), repo: crate::layout::repo_dir(&root), root }
    }

    /// The lock of the store checked out at `repo`: its state directory's
    /// if `repo` is `<state>/repo`, else the repository's own.
    pub fn for_repo(repo: &Path) -> Self {
        if let Some(state_dir) = crate::layout::state_dir_of(repo) {
            return Self::for_state_dir(state_dir);
        }
        let root = canonical(repo);
        let git = root.join(".git");
        let files = git.is_dir().then(|| git.join(GIT_LOCK_DIR));
        if let Some(files) = &files {
            let _ = std::fs::create_dir_all(files);
        }
        Self { repo: root.clone(), root, files }
    }

    /// The store's repository.
    pub fn repo(&self) -> &Path {
        &self.repo
    }

    fn store(&self) -> Lock {
        Lock { key: self.root.join("#store"), file: self.files.as_ref().map(|dir| dir.join(STORE_LOCK_FILE)) }
    }

    /// A lock between this process's threads only.
    fn in_process(&self, kind: &str) -> Lock {
        Lock { key: self.root.join(format!("#{kind}")), file: None }
    }

    /// Takes the store for changing it, waiting at most [`DEFAULT_WAIT`].
    pub fn exclusive(&self) -> Result<Exclusive, PawError> {
        self.exclusive_within(DEFAULT_WAIT)
    }

    /// [`Self::exclusive`], waiting at most `wait`.
    pub fn exclusive_within(&self, wait: Duration) -> Result<Exclusive, PawError> {
        let held = acquire(&self.store(), Mode::Exclusive, wait)?;
        if !held.nested {
            remove_stale_git_locks(&self.repo);
        }
        Ok(Exclusive { held, store: self.clone() })
    }

    /// Takes the store for reading it, waiting at most [`SHARED_WAIT`].
    /// Readers share; none overlaps a writer.
    pub fn shared(&self) -> Result<Shared, PawError> {
        self.shared_within(SHARED_WAIT)
    }

    /// [`Self::shared`], waiting at most `wait`.
    pub fn shared_within(&self, wait: Duration) -> Result<Shared, PawError> {
        Ok(Shared { _held: acquire(&self.store(), Mode::Shared, wait)? })
    }

    /// Takes the store's fetch lock: one fetch at a time writes objects and
    /// its private refs. In this process only (no file: it is held across
    /// the transfer); only the app fetches. Never taken while the store
    /// lock is held by this thread (the order is fetch lock, then store
    /// lock).
    pub(crate) fn fetching(&self, wait: Duration) -> Result<Fetching, PawError> {
        if holds(&self.store().key).is_some() {
            return Err(PawError::Store("paw asked for the fetch lock while holding the store".into()));
        }
        Ok(Fetching { _held: acquire(&self.in_process("fetch"), Mode::Exclusive, wait)? })
    }

    /// Takes the store's push lock, held by a send from before it reads the
    /// tip it will push until it has recorded the result: one send at a
    /// time, and [`Self::push_in_flight`] can tell a push is under way.
    /// Like the fetch lock, in this process only (only the app sends) and
    /// never taken while this thread holds the store.
    pub(crate) fn pushing(&self, wait: Duration) -> Result<Pushing, PawError> {
        if holds(&self.store().key).is_some() {
            return Err(PawError::Store("paw asked for the push lock while holding the store".into()));
        }
        Ok(Pushing { _held: acquire(&self.in_process("push"), Mode::Exclusive, wait)? })
    }

    /// Whether another thread holds the push lock now: a push is on the
    /// network. Only tries; never waits.
    pub(crate) fn push_in_flight(&self) -> bool {
        acquire(&self.in_process("push"), Mode::Exclusive, Duration::ZERO).is_err()
    }
}

/// The store, held for changing it. Functions that change a store take
/// one of these: proof, checked by the compiler, that the caller holds the
/// lock.
pub struct Exclusive {
    held: Held,
    store: StoreLock,
}

impl Exclusive {
    /// Whether this is the lock of the store checked out at `repo`.
    pub fn covers(&self, repo: &Path) -> bool {
        StoreLock::for_repo(repo).root == self.store.root
    }
}

impl std::fmt::Debug for Exclusive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Exclusive").field("store", &self.store.root).field("nested", &self.held.nested).finish()
    }
}

/// The store, held for reading it.
pub struct Shared {
    _held: Held,
}

/// A store's fetch lock, held.
pub(crate) struct Fetching {
    _held: Held,
}

/// A store's push lock, held.
pub(crate) struct Pushing {
    _held: Held,
}

// --- the two layers --------------------------------------------------------------------

#[derive(Default)]
struct GateState {
    readers: u32,
    writer: bool,
    writers_waiting: u32,
}

#[derive(Default)]
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}

static GATES: LazyLock<Mutex<HashMap<PathBuf, Weak<Gate>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn gate(key: &Path) -> Arc<Gate> {
    let mut gates = GATES.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(gate) = gates.get(key).and_then(Weak::upgrade) {
        return gate;
    }
    gates.retain(|_, gate| gate.strong_count() > 0);
    let gate = Arc::new(Gate::default());
    gates.insert(key.to_path_buf(), Arc::downgrade(&gate));
    gate
}

thread_local! {
    /// The locks this thread holds, with how.
    static HELD: RefCell<Vec<(PathBuf, Mode)>> = const { RefCell::new(Vec::new()) };
}

/// The strongest way this thread holds `key`.
fn holds(key: &Path) -> Option<Mode> {
    HELD.with(|held| {
        let held = held.borrow();
        let mut found = None;
        for (k, mode) in held.iter() {
            if k == key {
                if *mode == Mode::Exclusive {
                    return Some(Mode::Exclusive);
                }
                found = Some(Mode::Shared);
            }
        }
        found
    })
}

/// A lock held by this thread. Not `Send`: the thread-local record of what
/// is held must be cleared by the thread that made it.
struct Held {
    key: PathBuf,
    mode: Mode,
    gate: Option<Arc<Gate>>,
    file: Option<File>,
    nested: bool,
    _not_send: PhantomData<*const ()>,
}

fn acquire(lock: &Lock, mode: Mode, wait: Duration) -> Result<Held, PawError> {
    let record = |nested, gate, file| {
        HELD.with(|held| held.borrow_mut().push((lock.key.clone(), mode)));
        Held { key: lock.key.clone(), mode, gate, file, nested, _not_send: PhantomData }
    };
    match (holds(&lock.key), mode) {
        (Some(Mode::Exclusive), _) | (Some(Mode::Shared), Mode::Shared) => return Ok(record(true, None, None)),
        (Some(Mode::Shared), Mode::Exclusive) => {
            return Err(PawError::Store("paw tried to change the store while reading it".into()));
        }
        (None, _) => {}
    }
    let deadline = Instant::now() + wait;
    let gate = gate(&lock.key);
    enter(&gate, mode, deadline)?;
    match lock.file.as_deref().map(|path| lock_file(path, mode, deadline)).transpose() {
        Ok(file) => Ok(record(false, Some(gate), file.flatten())),
        Err(error) => {
            leave(&gate, mode);
            Err(error)
        }
    }
}

fn enter(gate: &Gate, mode: Mode, deadline: Instant) -> Result<(), PawError> {
    let mut state = gate.state.lock().unwrap_or_else(|p| p.into_inner());
    if mode == Mode::Exclusive {
        state.writers_waiting += 1;
    }
    loop {
        let free = match mode {
            Mode::Shared => !state.writer && state.writers_waiting == 0,
            Mode::Exclusive => !state.writer && state.readers == 0,
        };
        if free {
            break;
        }
        let now = Instant::now();
        if now >= deadline {
            if mode == Mode::Exclusive {
                state.writers_waiting -= 1;
                // Readers held back for this writer may go now.
                gate.changed.notify_all();
            }
            return Err(busy());
        }
        state = gate.changed.wait_timeout(state, deadline - now).unwrap_or_else(|p| p.into_inner()).0;
    }
    match mode {
        Mode::Shared => state.readers += 1,
        Mode::Exclusive => {
            state.writers_waiting -= 1;
            state.writer = true;
        }
    }
    Ok(())
}

fn leave(gate: &Gate, mode: Mode) {
    let mut state = gate.state.lock().unwrap_or_else(|p| p.into_inner());
    match mode {
        Mode::Shared => state.readers = state.readers.saturating_sub(1),
        Mode::Exclusive => state.writer = false,
    }
    gate.changed.notify_all();
}

/// Opens `path` and takes its `flock`, trying until `deadline`. Each holder
/// has its own open file, so a lock in this process conflicts with one in
/// another exactly as two processes' would; the in-process gate already
/// keeps this process's holders apart.
///
/// A reader that cannot create the lock file (a store it may not write,
/// which no paw process can then change either) reads without it.
fn lock_file(path: &Path, mode: Mode, deadline: Instant) -> Result<Option<File>, PawError> {
    let io = |e: std::io::Error| PawError::Store(format!("could not lock the store ({}): {e}", path.display()));
    let opened = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path));
    let file = match opened {
        Ok(file) => file,
        Err(e) if mode == Mode::Shared && e.kind() == std::io::ErrorKind::PermissionDenied => return Ok(None),
        Err(e) => return Err(io(e)),
    };
    let mut pause = Duration::from_millis(1);
    loop {
        let tried = match mode {
            Mode::Shared => file.try_lock_shared(),
            Mode::Exclusive => file.try_lock(),
        };
        match tried {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(e)) => return Err(io(e)),
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(busy());
        }
        std::thread::sleep(pause.min(deadline - now));
        pause = (pause * 2).min(Duration::from_millis(20));
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // Closing the file drops the flock; then the gate.
        drop(self.file.take());
        if let Some(gate) = self.gate.take() {
            leave(&gate, self.mode);
        }
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(at) = held.iter().rposition(|(k, m)| *k == self.key && *m == self.mode) {
                held.remove(at);
            }
        });
    }
}

// --- stale git locks ---------------------------------------------------------------------

/// Removes the `*.lock` files git left in `repo`'s `.git` when a process
/// was killed while writing: in `.git` itself (`index.lock`, `HEAD.lock`,
/// `config.lock`, `packed-refs.lock`, ...) and under `refs/` and `logs/`.
/// Called with the exclusive store lock held, which every writer of those
/// files in paw holds, so none of them is being written now. The private
/// refs of a fetch (`refs/paw/fetch/`) and `shallow.lock`, which are
/// written under the fetch lock instead, and paw's own lock files
/// (`.git/paw/`) are left alone.
/// Returns what was removed.
pub fn remove_stale_git_locks(repo: &Path) -> Vec<PathBuf> {
    let git = repo.join(".git");
    let mut removed = Vec::new();
    let Ok(top) = std::fs::read_dir(&git) else {
        return removed;
    };
    for child in top.flatten() {
        let path = child.path();
        // `shallow.lock` is a fetch's, written under the fetch lock alone.
        let fetchs = child.file_name() == SHALLOW_LOCK;
        if child.file_type().is_ok_and(|t| t.is_file()) && is_lock(&path) && !fetchs && std::fs::remove_file(&path).is_ok()
        {
            removed.push(path);
        }
    }
    let skip = crate::sync::refs::fetched_dirs(&git);
    for dir in [git.join("refs"), git.join("logs")] {
        remove_locks_under(&dir, &skip, 0, &mut removed);
    }
    removed
}

/// What libgit2 writes while a fetch updates `.git/shallow`.
const SHALLOW_LOCK: &str = "shallow.lock";

fn is_lock(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "lock")
}

fn remove_locks_under(dir: &Path, skip: &[PathBuf], depth: usize, removed: &mut Vec<PathBuf>) {
    // Ref names are short; a deeper tree is not git's.
    if depth > 32 || skip.iter().any(|s| s == dir) {
        return;
    }
    let Ok(children) = std::fs::read_dir(dir) else {
        return;
    };
    for child in children.flatten() {
        let path = child.path();
        let Ok(kind) = child.file_type() else { continue };
        if kind.is_dir() {
            remove_locks_under(&path, skip, depth + 1, removed);
        } else if kind.is_file() && is_lock(&path) && std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
}

/// Removes what a killed fetch left: its private refs' lock files and the
/// lock files of a pack index it was writing, and `shallow.lock`. Called
/// with the fetch lock held.
pub(crate) fn remove_stale_fetch_locks(repo: &Path) {
    let git = repo.join(".git");
    let _ = std::fs::remove_file(git.join(SHALLOW_LOCK));
    let mut removed = Vec::new();
    for dir in crate::sync::refs::fetched_dirs(&git) {
        remove_locks_under(&dir, &[], 0, &mut removed);
    }
    if let Ok(packs) = std::fs::read_dir(git.join("objects/pack")) {
        for child in packs.flatten() {
            let path = child.path();
            if is_lock(&path) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_that_holds_the_lock_gets_a_nested_guard_not_a_deadlock() {
        let dir = tempfile::tempdir().unwrap();
        let store = StoreLock::for_state_dir(dir.path());
        let outer = store.exclusive().unwrap();
        let inner = store.exclusive_within(Duration::ZERO).unwrap();
        let read = store.shared_within(Duration::ZERO).unwrap();
        assert!(inner.held.nested);
        drop((inner, read));
        // Still held: another thread cannot have it.
        let other = std::thread::spawn({
            let store = store.clone();
            move || store.shared_within(Duration::from_millis(50)).is_err()
        });
        assert!(other.join().unwrap());
        drop(outer);
        let other = std::thread::spawn(move || store.exclusive_within(Duration::from_millis(500)).is_ok());
        assert!(other.join().unwrap());
    }

    #[test]
    fn a_reader_asking_to_write_is_refused_not_deadlocked() {
        let dir = tempfile::tempdir().unwrap();
        let store = StoreLock::for_state_dir(dir.path());
        let _read = store.shared().unwrap();
        assert!(store.exclusive_within(Duration::from_secs(5)).is_err());
    }

    #[test]
    fn readers_share_and_writers_wait_for_them() {
        let dir = tempfile::tempdir().unwrap();
        let store = StoreLock::for_state_dir(dir.path());
        let reader = store.shared().unwrap();
        let other_reader = std::thread::spawn({
            let store = store.clone();
            move || store.shared_within(Duration::from_millis(500)).is_ok()
        });
        assert!(other_reader.join().unwrap());
        let writer = std::thread::spawn({
            let store = store.clone();
            move || store.exclusive_within(Duration::from_millis(50)).is_err()
        });
        assert!(writer.join().unwrap(), "a writer does not overlap a reader");
        drop(reader);
    }

    #[test]
    fn stale_git_locks_go_but_a_fetchs_private_refs_and_paws_own_files_stay() {
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        let git = dir.path().join(".git");
        let stale = [git.join("index.lock"), git.join("refs/heads/main.lock"), git.join("logs/HEAD.lock")];
        let kept = [git.join("refs/paw/fetch/main.lock"), git.join("objects/pack/pack-1.idx.lock")];
        for path in stale.iter().chain(&kept) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        let held = StoreLock::for_repo(dir.path()).exclusive().unwrap();
        assert!(stale.iter().all(|p| !p.exists()));
        assert!(kept.iter().all(|p| p.exists()));
        assert!(git.join(GIT_LOCK_DIR).join(STORE_LOCK_FILE).exists());
        drop(held);
    }

    #[test]
    fn a_push_in_flight_is_seen_from_another_thread() {
        let dir = tempfile::tempdir().unwrap();
        let store = StoreLock::for_state_dir(dir.path());
        assert!(!store.push_in_flight());
        let pushing = store.pushing(Duration::ZERO).unwrap();
        let other = std::thread::spawn({
            let store = store.clone();
            move || store.push_in_flight()
        });
        assert!(other.join().unwrap());
        drop(pushing);
        assert!(!store.push_in_flight());
        // Never under the store lock.
        let _held = store.exclusive().unwrap();
        assert!(store.pushing(Duration::ZERO).is_err());
        assert!(store.fetching(Duration::ZERO).is_err());
    }

    #[test]
    fn a_repository_named_repo_shares_its_state_directorys_lock() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let held = StoreLock::for_state_dir(dir.path()).exclusive().unwrap();
        assert!(held.covers(&repo));
        let elsewhere = tempfile::tempdir().unwrap();
        git2::Repository::init(elsewhere.path()).unwrap();
        assert!(!held.covers(elsewhere.path()));
        let other = StoreLock::for_repo(elsewhere.path()).exclusive().unwrap();
        assert!(other.covers(elsewhere.path()));
        assert!(elsewhere.path().join(".git/paw").join(STORE_LOCK_FILE).exists());
    }
}

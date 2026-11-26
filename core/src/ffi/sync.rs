//! Setup and sync as Swift sees them: the setup screens drive [`Setup`];
//! the sync footer, the attention row and the "not yet pushed" screen use
//! [`StoreSync`].
//!
//! The steps that need the key run as a [`KeyTask`] on the [`KeyPort`]'s
//! session (the `*_with_key` calls): the key sheet binds to the task, the
//! PIN is asked at most once while the key stays plugged in, `Touch` and
//! progress are the task's stages, and `cancel` stops the work between
//! network steps. No call here takes a PIN. A task that fails ends in the
//! state that says what failed: `Failed` for the key, `SyncFailed` or
//! `SetupFailed` for the work after it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use crate::key::{
    After, DecryptWork, Halt, KeyError, KeyIdentity, KeyOutcome, KeyPort, KeyPurpose, KeySession, KeyTask,
    KeyTaskListener, KeyTaskState, KeyTouch, LoginPurpose, SessionSigner, TaskCore, TaskFailure, TaskProgress, Work,
};
use crate::key::touch_clock::{TouchClock, TouchExpectation};
use crate::setup::{SetupError, SetupFlow, SetupState, SetupWriteReadiness};
use crate::store::PassStore;
use crate::remote::RemoteFailure;
use crate::sync::{ConnectError, Connection, Route};
use crate::sync::address::{StoreAddress, saved_address};
use crate::sync::hosts::{KnownHosts, is_bundled};
use crate::sync::operation;
use crate::sync::queue::{self, HeldBack, OpenFailure, QueueItem, ReencryptCheck};
use crate::sync::{IntegrateReport, RefusedPath, SyncError, TransferProgress, accept_replaced_remote};

/// First-run setup; see [`crate::setup`]. Each call is one step and returns
/// the state to show next.
#[derive(uniffi::Object)]
pub struct Setup {
    flow: Mutex<SetupFlow>,
    /// How far the last download's clone got. Outside `flow`, so it
    /// answers while the download runs.
    transfer: Arc<Mutex<Option<TransferProgress>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Plain values; a panic cannot leave them half written.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Setup {
    /// Connections go where `route` says (a simulation playing a host).
    #[cfg(feature = "sim")]
    pub(crate) fn set_route(&self, route: Route) {
        self.flow().set_route(route);
    }

    fn flow(&self) -> MutexGuard<'_, SetupFlow> {
        // A panic in a step cannot leave the saved state half written: it is
        // written whole, by rename.
        lock(&self.flow)
    }

    /// The download, in a sign-in task's thread, once the PIN is verified.
    /// The setup state is locked only between network steps, so `state`
    /// answers while it runs.
    fn run_download(&self, task: &Arc<TaskCore>, signer: &SessionSigner) -> Result<KeyOutcome, TaskFailure> {
        let cancelled = || TaskFailure::Key(KeyError::Cancelled);
        let (remote, repo) = self.flow().begin_download()?;
        if task.cancelled() {
            self.flow().abandon_download();
            return Err(cancelled());
        }
        let connection = match Connection::open(&remote, signer) {
            Ok(connection) => connection,
            Err(error) => {
                let mut flow = self.flow();
                flow.abandon_download();
                if task.cancelled() {
                    return Err(cancelled());
                }
                return Err(match error {
                    ConnectError::Key(error) => error.into(),
                    ConnectError::Sync(error) => SetupError::from(error).into(),
                });
            }
        };
        self.flow().note_login();
        let mut progress = |update: TransferProgress| {
            if task.cancelled() {
                return false;
            }
            *lock(&self.transfer) = Some(update);
            task.report_progress(update.received_objects, update.total_objects)
        };
        let cloned = crate::sync::clone_with_transfer(&connection, &remote, &repo, Some(&mut progress));
        drop(connection);
        if !task.begin_commit() {
            self.flow().abandon_download();
            return Err(cancelled());
        }
        self.flow().finish_download(cloned)?;
        Ok(KeyOutcome::SignedIn)
    }
}

#[uniffi::export]
impl Setup {
    /// Opens the setup kept in `state_dir` and resumes where it stopped.
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Result<Arc<Self>, SetupError> {
        let flow = SetupFlow::open(&PathBuf::from(state_dir))?;
        Ok(Arc::new(Self { flow: Mutex::new(flow), transfer: Arc::default() }))
    }

    pub fn state(&self) -> SetupState {
        self.flow().state()
    }

    pub fn back(&self) -> Result<SetupState, SetupError> {
        self.flow().back()
    }

    pub fn set_address(&self, address: String) -> Result<SetupState, SetupError> {
        self.flow().set_address(&address)
    }

    /// Checks the host and whether it accepts the key. No PIN, no touch.
    pub fn check_host(&self) -> Result<SetupState, SetupError> {
        self.flow().check_host()
    }

    /// Pins the host key whose fingerprint `ConfirmHost` showed.
    pub fn confirm_host(&self, fingerprint: String) -> Result<SetupState, SetupError> {
        self.flow().confirm_host(&fingerprint)
    }

    /// "Your key", from the plugged-in key's session: what the session read
    /// when the key was plugged in. No PIN, no touch, no card exchange. A
    /// key whose PIN KDF paw does not support is refused here with
    /// `UnusableKey` (`PinKdfUnsupported`), before any PIN is asked.
    pub fn read_key_from_session(&self, session: Arc<KeySession>) -> Result<SetupState, SetupError> {
        let ssh = session.ssh_key().ok().cloned();
        self.flow().read_key_from(session.card_info(), ssh.as_ref(), session.pin_kdf())
    }

    /// "Download your store" as a key task (sign-in, `Clone`): the PIN
    /// stage unless the session holds it, `Working` while connecting,
    /// `Touch` while the key signs the login, `Working` with progress while
    /// cloning, then `Done`; read `state` for the next step. A key other
    /// than the one read at "Your key" is refused before the PIN.
    pub fn download_with_key(self: Arc<Self>, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        *lock(&self.transfer) = None;
        let expected = self.flow().expected_key();
        let setup = self.clone();
        let work: Work = Box::new(move |task, signer| setup.run_download(task, &signer));
        port.sign_in_then(LoginPurpose::Clone, listener, expected, work)
    }

    /// "Check your key" as a key task (`CheckKey`): opens one entry with
    /// the held PIN and a touch, wipes the text, then pins the card's key
    /// for saving, accepts the store's recipients, records the key as the
    /// store's enrolled key (and gives it to `port`), and setup is done.
    pub fn test_decrypt_with_key(self: Arc<Self>, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        let (picked, repo, expected) = {
            let flow = self.flow();
            (flow.pick_test_entry(), flow.repo_dir(), flow.expected_key())
        };
        let opened = picked.and_then(|entry| {
            let store = PassStore::open(repo.to_string_lossy().into_owned()).map_err(SetupError::from)?;
            Ok((entry, store))
        });
        let (entry, store) = match opened {
            Ok(found) => found,
            Err(error) => return KeyTask::failed(KeyPurpose::CheckKey { entry: None }, listener, error.into()),
        };
        let setup = self.clone();
        let enrol = port.clone();
        let opened_entry = entry.clone();
        let after: After = Box::new(move |session| {
            let key = session.decryption_key()?;
            let identity = KeyIdentity::of(session.card_info());
            setup.flow().finish_test_decrypt(opened_entry, key, &identity)?;
            enrol.set_enrolled_key(Some(identity));
            Ok(())
        });
        port.check_key_then(store, entry, listener, expected, after)
    }

    /// "I expected this change", on the changed-host-key screen: forgets
    /// the host's pinned key without leaving setup; `check_host` then shows
    /// the key it presents now for confirmation. Refused (`Failed`) for a
    /// host whose keys ship with paw, such as github.com.
    pub fn forget_host_key(&self) -> Result<SetupState, SetupError> {
        self.flow().forget_host_key()
    }

    /// How far the current or last download's clone has got: objects and
    /// bytes received, updated at most ten times a second. `None` before
    /// its transfer starts. Answers while a download runs.
    pub fn transfer_progress(&self) -> Option<TransferProgress> {
        *lock(&self.transfer)
    }

    /// After `RemoteFailure::NoConnection` with a `fallback` (no
    /// connection at all to github.com on port 22): switches the address to
    /// that endpoint and goes back to the host check (`CheckHost`, with a
    /// notice saying why); call `check_host` next. Its keys ship with paw,
    /// so nothing is confirmed. Never used unless the person says so.
    pub fn use_port_fallback(&self) -> Result<SetupState, SetupError> {
        self.flow().use_port_fallback()
    }

    /// Once setup is done: whether new entries can be saved with no
    /// further step, and if not, which recipients paw needs keys for.
    pub fn write_readiness(&self) -> Result<SetupWriteReadiness, SetupError> {
        self.flow().write_readiness()
    }
}

/// Keeping a set-up store in step with its remote.
#[derive(uniffi::Object)]
pub struct StoreSync {
    state_dir: PathBuf,
    address: StoreAddress,
    /// When the authentication slot last signed, for a cached touch policy.
    last_login: Mutex<Option<Instant>>,
}

impl StoreSync {
    fn repo(&self) -> PathBuf {
        crate::layout::repo_dir(&self.state_dir)
    }

    fn known_hosts(&self) -> KnownHosts {
        KnownHosts::of(&self.state_dir)
    }

    fn remote(&self) -> Result<crate::sync::Remote, SyncError> {
        self.known_hosts().remote(&self.address)?.ok_or_else(|| {
            RemoteFailure::NoSavedHostKey { host: self.address.connect_host().to_owned() }.into()
        })
    }

    fn clock(&self, policy: KeyTouch) -> TouchClock {
        let mut clock = TouchClock::new(policy);
        if let Some(at) = *self.last_login.lock().unwrap_or_else(|p| p.into_inner()) {
            clock.used_at(at);
        }
        clock
    }

    /// A sync as a sign-in task on the key session; see the module comment.
    fn with_key(
        self: Arc<Self>,
        port: Arc<KeyPort>,
        listener: Arc<dyn KeyTaskListener>,
        login: LoginPurpose,
        push: bool,
    ) -> Arc<KeyTask> {
        let sync = self.clone();
        let work: Work = Box::new(move |task, signer| {
            let remote = sync.remote()?;
            let stop = || task.cancelled();
            let mut progress =
                |update: TransferProgress| task.report_progress(update.received_objects, update.total_objects);
            let result: Result<_, TaskFailure> =
                operation::run_until(&remote, &signer, &sync.repo(), &sync.state_dir, push, &stop, Some(&mut progress));
            if task.cancelled() {
                return Err(KeyError::Cancelled.into());
            }
            let report = result?;
            if report.signatures > 0 {
                *lock(&sync.last_login) = Some(Instant::now());
            }
            Ok(KeyOutcome::Synced { report })
        });
        let expected = port.expected();
        port.sign_in_then(login, listener, expected, work)
    }
}

/// One host key the store's server must present.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ServerKey {
    /// "Ed25519", "ECDSA P-256", "RSA".
    pub key_type: String,
    /// `SHA256:...`, as `ssh-keygen -lf` and the host's documentation show it.
    pub fingerprint: String,
}

/// The store's server and the keys paw accepts from it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ServerKeys {
    pub host: String,
    pub port: u16,
    /// The keys ship with paw; they cannot be forgotten.
    pub bundled: bool,
    pub keys: Vec<ServerKey>,
}

#[uniffi::export]
impl StoreSync {
    /// Opens the store set up in `state_dir`.
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Result<Arc<Self>, SyncError> {
        let state_dir = PathBuf::from(state_dir);
        let address = saved_address(&state_dir)?;
        Ok(Arc::new(Self { state_dir, address, last_login: Mutex::new(None) }))
    }

    /// The store's address, as shown in Settings.
    pub fn address(&self) -> String {
        self.address.display()
    }

    /// The fingerprints of the host keys the store's host must present.
    pub fn host_fingerprints(&self) -> Result<Vec<String>, SyncError> {
        Ok(self.remote()?.host_key.fingerprints())
    }

    /// Settings › Store › server keys: the host, whether its keys ship
    /// with paw (and so cannot be forgotten), and each pinned key's type
    /// and fingerprint. A forgotten key leaves `keys` empty until the next
    /// connection is confirmed.
    pub fn server_keys(&self) -> Result<ServerKeys, SyncError> {
        let host = self.address.connect_host();
        let pin = self.known_hosts().pin(host, self.address.port)?;
        Ok(ServerKeys {
            host: host.to_owned(),
            port: self.address.port,
            bundled: is_bundled(host, self.address.port),
            keys: pin
                .map(|pin| {
                    pin.described()
                        .into_iter()
                        .map(|(key_type, fingerprint)| ServerKey { key_type, fingerprint })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// After Forget: connects to the store's server, without the key and
    /// without logging in, and returns the key it presents, for the person
    /// to compare before [`Self::trust_server_key`]. Network only.
    pub fn check_server_key(&self) -> Result<ServerKey, SyncError> {
        let host = self.address.connect_host();
        let presented = crate::sync::scan_host_key(host, self.address.port, &Route::default())?;
        Ok(ServerKey {
            key_type: crate::sync::key_type_words(&presented.algorithm()),
            fingerprint: presented.fingerprint(),
        })
    }

    /// The person compared `fingerprint` and trusts it: connects again and
    /// pins the key the server presents, if it is the one with that
    /// fingerprint. Refused for a host whose keys ship with paw, and for a
    /// host that already has a saved key (forget it first).
    pub fn trust_server_key(&self, fingerprint: String) -> Result<(), SyncError> {
        let (host, port) = (self.address.connect_host(), self.address.port);
        let scan = || crate::sync::scan_host_key(host, port, &Route::default());
        if !self.known_hosts().trust(host, port, scan, &fingerprint)? {
            return Err(RemoteFailure::HostKeyNotConfirmed { host: host.to_owned() }.into());
        }
        Ok(())
    }

    /// Settings › Store › Forget server key. The next setup or sync of a
    /// host that is not bundled asks to confirm its key again. A bundled
    /// host's keys (github.com's, ...) cannot be forgotten: `Failed`.
    pub fn forget_host_key(&self) -> Result<(), SyncError> {
        self.known_hosts().forget(self.address.connect_host(), self.address.port)
    }

    /// Whether the next login will wait for a touch, given the slot's policy.
    pub fn touch_expectation(&self, policy: KeyTouch) -> TouchExpectation {
        self.clock(policy).expectation()
    }

    /// "Send" as a key task (sign-in, `Push`): fetch, integrate and push on
    /// one login, with the session's PIN. Ends `Done` with `Synced` and the
    /// report, or `SyncFailed`. A queue that can't be read fails it at once.
    pub fn send_with_key(self: Arc<Self>, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        match queue::unpushed(&self.repo()) {
            Ok(items) => self.with_key(port, listener, LoginPurpose::Push { count: items.len() as u32 }, true),
            Err(error) => {
                let purpose = KeyPurpose::SignIn { login: LoginPurpose::Push { count: 0 } };
                KeyTask::failed(purpose, listener, error.into())
            }
        }
    }

    /// Fetch and integrate as a key task (sign-in, `Fetch`), pushing
    /// nothing. Ends as `send_with_key` does.
    pub fn refresh_with_key(self: Arc<Self>, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        self.with_key(port, listener, LoginPurpose::Fetch, false)
    }

    /// The "not yet pushed" list, newest first.
    pub fn unpushed(&self) -> Result<Vec<QueueItem>, SyncError> {
        queue::unpushed(&self.repo())
    }

    /// The waiting changes "Send" will not push, because their ciphertext
    /// is not encrypted for exactly the recipients their entry has now (its
    /// `.gpg-id` changed, or the change is not accepted yet). While any is
    /// listed, a send fetches and integrates but pushes nothing and ends
    /// `Waiting`. Show each entry with who it is encrypted for and who it
    /// should be for; the person accepts the new recipients, then
    /// re-encrypts them with `reencrypt_with_key`, or discards one outright.
    pub fn held_back(&self) -> Result<Vec<HeldBack>, SyncError> {
        queue::held_back(&self.repo(), &self.state_dir)
    }

    /// How many held-back changes "Re-encrypt" can do now (their folder's
    /// new list is accepted and paw has every key on it), and why each
    /// other one can't be done. No key, no network.
    pub fn reencrypt_check(&self) -> Result<ReencryptCheck, SyncError> {
        queue::reencrypt_check(&self.repo(), &self.state_dir)
    }

    /// "Re-encrypt" as a key task (`Reencrypt`): opens each held-back
    /// change that is ready with the key (the PIN once, a touch each) and
    /// encrypts it for its folder's new list, in place in the queue. Shows
    /// `Working` with (done, total) before each one, then `Touch`. Each one
    /// is written as soon as it is done, so cancel (or a key error) part-way
    /// keeps those and leaves the rest held back; running it again goes on
    /// from there. Nothing is sent: afterwards they are ordinary waiting
    /// changes. Ends `Done` with `Reencrypted` and what it did and left if
    /// it ran to the end; `reencrypt_check` says what is left after a stop.
    pub fn reencrypt_with_key(self: Arc<Self>, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        let count = match queue::reencrypt_check(&self.repo(), &self.state_dir) {
            Ok(check) => check.ready,
            Err(error) => return KeyTask::failed(KeyPurpose::Reencrypt { count: 0 }, listener, error.into()),
        };
        let sync = self.clone();
        let work: DecryptWork = Box::new(move |task, opener| {
            let mut open = |_: &HeldBack, ciphertext: &[u8]| {
                if task.cancelled() {
                    return Err(OpenFailure::from(KeyError::Cancelled));
                }
                opener(ciphertext).map_err(|halt| match halt {
                    Halt::Fail(error) => OpenFailure::from(error),
                    halt => OpenFailure::Stop(halt.into()),
                })
            };
            let mut progress = |done: u32, total: u32| {
                task.set(KeyTaskState::Working { progress: Some(TaskProgress { done, total }) });
            };
            let result = queue::reencrypt_each(&sync.repo(), &sync.state_dir, &mut open, &mut progress);
            if task.cancelled() {
                return Err(KeyError::Cancelled.into());
            }
            Ok(KeyOutcome::Reencrypted { report: result? })
        });
        port.decrypt_then(KeyPurpose::Reencrypt { count }, listener, work)
    }

    pub fn discard(&self, id: String) -> Result<Vec<QueueItem>, SyncError> {
        queue::discard(&self.repo(), &id)
    }

    pub fn undo_last(&self) -> Result<Vec<QueueItem>, SyncError> {
        queue::undo_last(&self.repo())
    }

    pub fn discard_all(&self) -> Result<(), SyncError> {
        queue::discard_all(&self.repo())
    }

    /// What the fetched remote holds that paw will not check out (links,
    /// submodules, unsafe names, names that are the same on an iPhone),
    /// with why; empty if nothing. A sync that fails with
    /// `RemoteFailure::RefusedContent` names the same paths.
    pub fn refused_remote(&self) -> Result<Vec<RefusedPath>, SyncError> {
        crate::sync::refused_remote(&self.repo())
    }

    /// After `RemoteReplaced`: take the remote's history, keeping or
    /// discarding this device's unpushed changes.
    pub fn accept_replaced_remote(&self, keep_changes: bool) -> Result<IntegrateReport, SyncError> {
        accept_replaced_remote(&self.repo(), keep_changes)
    }
}

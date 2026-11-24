//! Setup and sync as Swift sees them: the setup screens drive [`Setup`];
//! the sync footer, the attention row and the "not yet pushed" screen use
//! [`StoreSync`].
//!
//! The steps that need the key run as a [`KeyTask`] on the [`KeyPort`]'s
//! session (the `*_with_key` calls): the key sheet binds to the task, the
//! PIN is asked at most once while the key stays plugged in, `Touch` and
//! progress are the task's stages, and `cancel` stops the work between
//! network steps. No call here takes a PIN. When such a task fails for a reason that is not
//! the key's (the host, the repository, the store), its error is
//! `KeyError::Reader` with the reason as text, and `last_task_result`
//! returns the structured error.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use super::{Adapter, CardConnection};
use crate::key::{
    After, Halt, KeyError, KeyIdentity, KeyOutcome, KeyPort, KeyPurpose, KeySession, KeyTask, KeyTaskListener,
    LoginPurpose, SessionSigner, TaskCore, Work,
};
use crate::setup::address::StoreAddress;
use crate::setup::touch::{TouchClock, TouchExpectation};
use crate::setup::{SetupError, SetupFlow, SetupState, SetupWriteReadiness};
use crate::store::PassStore;
use crate::sync::{Cloned, Connection};
use crate::{KeyTouch, PawError};
use crate::sync::hosts::pin_for;
use crate::sync::operation::{self, SyncReport};
use crate::sync::queue::{self, HeldBack, QueueItem};
use crate::sync::{IntegrateReport, RefusedPath, SyncError, TransferProgress, accept_replaced_remote};

/// First-run setup; see [`crate::setup`]. Each call is one step and returns
/// the state to show next.
#[derive(uniffi::Object)]
pub struct Setup {
    flow: Mutex<SetupFlow>,
    /// Why the last key task setup started failed, if for a setup reason.
    last_error: Mutex<Option<SetupError>>,
    /// How far the last download's clone got. Outside `flow`, so it
    /// answers while the download runs.
    transfer: Arc<Mutex<Option<TransferProgress>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Plain values; a panic cannot leave them half written.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The task's error for a failure that is not the key's: the sheet shows
/// the text; the structured error is in `last_task_result`.
fn elsewhere(error: &impl std::fmt::Display) -> Halt {
    Halt::Fail(KeyError::Reader { detail: error.to_string() })
}

impl Setup {
    fn flow(&self) -> MutexGuard<'_, SetupFlow> {
        // A panic in a step cannot leave the saved state half written: it is
        // written whole, by rename.
        lock(&self.flow)
    }

    fn record(&self, error: SetupError) -> Halt {
        let halt = elsewhere(&error);
        *lock(&self.last_error) = Some(error);
        halt
    }

    /// The download, in a sign-in task's thread, once the PIN is verified.
    /// The setup state is locked only between network steps, so `state`
    /// answers while it runs.
    fn run_download(&self, task: &Arc<TaskCore>, signer: &SessionSigner) -> Result<Option<KeyOutcome>, Halt> {
        let cancelled = || Halt::Fail(KeyError::Cancelled);
        let (remote, repo) = self.flow().begin_download().map_err(|e| self.record(e))?;
        if task.cancelled() {
            let _ = self.flow().abandon_download();
            return Err(cancelled());
        }
        let connection = match Connection::open(&remote, signer) {
            Ok(connection) => connection,
            Err(error) => {
                let mut flow = self.flow();
                let _ = flow.abandon_download();
                if task.cancelled() {
                    return Err(cancelled());
                }
                return Err(match error {
                    e @ (PawError::Network(_) | PawError::HostKeyMismatch { .. } | PawError::KeyRejected) => {
                        let error = flow.connection_error(&remote, e, None);
                        drop(flow);
                        self.record(error)
                    }
                    // The signer has already failed the task with the key's
                    // own error (touch timeout, removed, PIN forgotten).
                    other => elsewhere(&other),
                });
            }
        };
        self.flow().note_login();
        let mut progress = |update: TransferProgress| {
            if task.cancelled() {
                return false;
            }
            *lock(&self.transfer) = Some(update);
            task.report_transfer(update)
        };
        let cloned = crate::sync::clone_with_transfer(&connection, &remote, &repo, Some(&mut progress));
        let said = connection.remote_said();
        drop(connection);
        if !task.begin_commit() {
            let _ = self.flow().abandon_download();
            return Err(cancelled());
        }
        let cloned: Result<Cloned, PawError> = cloned;
        self.flow().finish_download(&remote, cloned, said).map_err(|e| self.record(e))?;
        Ok(Some(KeyOutcome::SignedIn))
    }
}

#[uniffi::export]
impl Setup {
    /// Opens the setup kept in `state_dir` and resumes where it stopped.
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Result<Arc<Self>, SetupError> {
        let flow = SetupFlow::open(&PathBuf::from(state_dir))?;
        Ok(Arc::new(Self { flow: Mutex::new(flow), last_error: Mutex::new(None), transfer: Arc::default() }))
    }

    pub fn state(&self) -> SetupState {
        self.flow().state()
    }

    pub fn back(&self) -> Result<SetupState, SetupError> {
        self.flow().back()
    }

    /// Reads the plugged-in key. No PIN, no touch.
    pub fn read_key(&self, connection: Arc<dyn CardConnection>) -> Result<SetupState, SetupError> {
        self.flow().read_key(Arc::new(Adapter(connection)))
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
    /// `UnusableKey` ("This key uses a PIN setting (KDF) that paw doesn't
    /// support yet"), before any PIN is asked.
    pub fn read_key_from_session(&self, session: Arc<KeySession>) -> Result<SetupState, SetupError> {
        let ssh = session.ssh_key().ok().cloned();
        self.flow().read_key_from(session.card_info(), ssh.as_ref(), session.pin_kdf_supported())
    }

    /// "Download your store" as a key task (sign-in, `Clone`): the PIN
    /// stage unless the session holds it, `Working` while connecting,
    /// `Touch` while the key signs the login, `Working` with progress while
    /// cloning, then `Done`; read `state` for the next step. A key other
    /// than the one read at "Your key" is refused before the PIN.
    pub fn download_with_key(self: Arc<Self>, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        *lock(&self.last_error) = None;
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
        *lock(&self.last_error) = None;
        let (picked, repo, expected) = {
            let flow = self.flow();
            (flow.pick_test_entry(), flow.repo_dir(), flow.expected_key())
        };
        let opened = picked.and_then(|entry| {
            let store = PassStore::open(repo.to_string_lossy().into_owned())
                .map_err(|e| SetupError::Failed { message: e.to_string() })?;
            Ok((entry, store))
        });
        let (entry, store) = match opened {
            Ok(found) => found,
            Err(error) => {
                let purpose = KeyPurpose::CheckKey { entry: String::new() };
                let key_error = KeyError::Reader { detail: error.to_string() };
                *lock(&self.last_error) = Some(error);
                return KeyTask::failed(purpose, listener, key_error);
            }
        };
        let setup = self.clone();
        let enrol = port.clone();
        let opened_entry = entry.clone();
        let after: After = Box::new(move |session| {
            let key = session.decryption_key()?;
            let identity = KeyIdentity::of(session.card_info());
            let finished = setup.flow().finish_test_decrypt(opened_entry, key, &identity);
            match finished {
                Ok(_) => {
                    enrol.set_enrolled_key(Some(identity));
                    Ok(())
                }
                Err(error) => {
                    let detail = error.to_string();
                    *lock(&setup.last_error) = Some(error);
                    Err(KeyError::Reader { detail })
                }
            }
        });
        port.check_key_then(store, entry, listener, expected, after)
    }

    /// After a `*_with_key` task: the setup error it failed with, if it
    /// failed for a reason that is not the key's (host, repository, store),
    /// else the state to show now.
    pub fn last_task_result(&self) -> Result<SetupState, SetupError> {
        match lock(&self.last_error).clone() {
            Some(error) => Err(error),
            None => Ok(self.state()),
        }
    }

    /// After a download failed with "the remote contains something paw
    /// will not check out": the paths it refused and why, for the screen
    /// that says so. Empty otherwise. Nothing was checked out.
    pub fn refused_remote(&self) -> Vec<RefusedPath> {
        self.flow().refused_remote()
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

    /// After a host check or download that made no connection at all to
    /// github.com on port 22 (refused or timed out, not a changed host key
    /// or a refused key): GitHub's port-443 endpoint, to offer as "Port 22
    /// seems blocked. Try GitHub's port 443?". Never used unless the person
    /// says so with `use_port_fallback`. `None` otherwise.
    pub fn port_fallback(&self) -> Option<crate::setup::PortFallback> {
        self.flow().port_fallback()
    }

    /// Switches the address to the `port_fallback` endpoint and goes back
    /// to the host check (`CheckHost`, with a notice saying why); call
    /// `check_host` next. Its keys ship with paw, so nothing is confirmed.
    pub fn use_port_fallback(&self) -> Result<SetupState, SetupError> {
        self.flow().use_port_fallback()
    }

    /// After a host check or a download that failed: what it found out
    /// about the remote, to pick the screen ("No store at that address" for
    /// `RepositoryNotFound`, which comes as `SetupError::Failed`). `None`
    /// if the last step went through, or failed for the key's or the
    /// store's sake.
    pub fn last_remote_message(&self) -> Option<crate::setup::outcome::RemoteMessage> {
        self.flow().last_remote_message()
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
    /// How the last `*_with_key` task's sync went, once it has.
    last_result: Mutex<Option<Result<SyncReport, SyncError>>>,
}

impl StoreSync {
    fn repo(&self) -> PathBuf {
        crate::setup::repo_dir(&self.state_dir)
    }

    fn remote(&self) -> Result<crate::sync::Remote, SyncError> {
        let host = self.address.connect_host();
        let pin = pin_for(&crate::setup::known_hosts(&self.state_dir), host, self.address.port)?
            .ok_or_else(|| SyncError::Failed { message: format!("{host} has no saved key; set the store up again") })?;
        Ok(self.address.remote(pin))
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
        *lock(&self.last_result) = None;
        let sync = self.clone();
        let work: Work = Box::new(move |task, signer| {
            let remote = match sync.remote() {
                Ok(remote) => remote,
                Err(error) => {
                    let halt = elsewhere(&error);
                    *lock(&sync.last_result) = Some(Err(error));
                    return Err(halt);
                }
            };
            let stop = || task.cancelled();
            let mut progress = |update: TransferProgress| task.report_transfer(update);
            let result =
                operation::run_until(&remote, &signer, &sync.repo(), &sync.state_dir, push, &stop, Some(&mut progress));
            if task.cancelled() {
                return Err(Halt::Fail(KeyError::Cancelled));
            }
            if result.as_ref().is_ok_and(|r| r.signatures > 0) {
                *lock(&sync.last_login) = Some(Instant::now());
            }
            let halt = result.as_ref().err().map(elsewhere);
            *lock(&sync.last_result) = Some(result);
            match halt {
                // A signing failure has already failed the task with the
                // key's own error; this one is then ignored.
                Some(halt) => Err(halt),
                None => Ok(Some(KeyOutcome::SignedIn)),
            }
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
        let address = crate::setup::saved_address(&state_dir)?;
        Ok(Arc::new(Self { state_dir, address, last_login: Mutex::new(None), last_result: Mutex::new(None) }))
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
        let pin = pin_for(&crate::setup::known_hosts(&self.state_dir), host, self.address.port)?;
        Ok(ServerKeys {
            host: host.to_owned(),
            port: self.address.port,
            bundled: crate::sync::hosts::is_bundled(host, self.address.port),
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
        let presented = crate::sync::scan_host_key(host, self.address.port)?;
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
        let host = self.address.connect_host();
        if crate::sync::hosts::is_bundled(host, self.address.port) {
            return Err(SyncError::Failed { message: crate::setup::bundled_refusal(host) });
        }
        let presented = crate::sync::scan_host_key(host, self.address.port)?;
        if presented.fingerprint() != fingerprint.trim() {
            return Err(SyncError::Failed {
                message: format!("{host} now shows a different key from the one you checked"),
            });
        }
        let line = presented.line()?;
        Ok(crate::setup::known_hosts(&self.state_dir).add(host, self.address.port, &line)?)
    }

    /// Settings › Store › Forget server key. The next setup or sync of a
    /// host that is not bundled asks to confirm its key again. A bundled
    /// host's keys (github.com's, ...) cannot be forgotten: `Failed`.
    pub fn forget_host_key(&self) -> Result<(), SyncError> {
        let host = self.address.connect_host();
        if crate::sync::hosts::is_bundled(host, self.address.port) {
            return Err(SyncError::Failed { message: crate::setup::bundled_refusal(host) });
        }
        let known = crate::setup::known_hosts(&self.state_dir);
        Ok(known.forget(host, self.address.port)?)
    }

    /// Whether the next login will wait for a touch, given the slot's policy.
    pub fn touch_expectation(&self, policy: KeyTouch) -> TouchExpectation {
        self.clock(policy).expectation()
    }

    /// "Send" as a key task (sign-in, `Push`): fetch, integrate and push on
    /// one login, with the session's PIN. `Done` means it succeeded; read
    /// `last_task_result` for the report.
    pub fn send_with_key(self: Arc<Self>, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        let count = queue::unpushed(&self.repo()).map(|items| items.len() as u32).unwrap_or(0);
        self.with_key(port, listener, LoginPurpose::Push { count }, true)
    }

    /// Fetch and integrate as a key task (sign-in, `Fetch`), pushing
    /// nothing.
    pub fn refresh_with_key(self: Arc<Self>, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        self.with_key(port, listener, LoginPurpose::Fetch, false)
    }

    /// After a `*_with_key` task: its report, or the sync error it failed
    /// with if it failed for a reason that is not the key's. `None` if no
    /// such task has finished its sync (it was cancelled, or failed at the
    /// key).
    pub fn last_task_result(&self) -> Result<Option<SyncReport>, SyncError> {
        lock(&self.last_result).clone().transpose()
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
    /// should be for; the person accepts the new recipients, then saves the
    /// entry again (re-encrypting it, which needs the key) and discards the
    /// old waiting change, or discards it outright.
    pub fn held_back(&self) -> Result<Vec<HeldBack>, SyncError> {
        queue::held_back(&self.repo(), &self.state_dir)
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
    /// with why; empty if nothing. When a sync fails with a message
    /// starting "the remote contains something paw will not check out",
    /// show these: the store and the unpushed changes were left as they
    /// were, and the remote has to be fixed from another computer.
    pub fn refused_remote(&self) -> Result<Vec<RefusedPath>, SyncError> {
        crate::sync::refused_remote(&self.repo())
    }

    /// After `RemoteReplaced`: take the remote's history, keeping or
    /// discarding this device's unpushed changes.
    pub fn accept_replaced_remote(&self, keep_changes: bool) -> Result<IntegrateReport, SyncError> {
        accept_replaced_remote(&self.repo(), keep_changes)
    }
}

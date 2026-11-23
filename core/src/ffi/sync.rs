//! Setup and sync as Swift sees them: the setup screens drive [`Setup`];
//! the sync footer, the attention row and the "not yet pushed" screen use
//! [`StoreSync`].
//!
//! The steps that need the key come in two forms. The older ones take a
//! [`CardConnection`] and the PIN per call, and call `touch_needed` only
//! when a touch is, or may be, needed. The `*_with_key` ones run as a
//! [`KeyTask`] on the [`KeyPort`]'s session instead: the key sheet binds to
//! the task, the PIN is asked at most once while the key stays plugged in,
//! `Touch` and progress are the task's stages, and `cancel` stops the work
//! between network steps. When such a task fails for a reason that is not
//! the key's (the host, the repository, the store), its error is
//! `KeyError::Reader` with the reason as text, and `last_task_result`
//! returns the structured error.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use zeroize::Zeroizing;

use super::{Adapter, CardConnection};
use crate::key::{
    After, Halt, KeyError, KeyIdentity, KeyOutcome, KeyPort, KeyPurpose, KeySession, KeyTask, KeyTaskListener,
    KeyTaskState, LoginPurpose, SessionSigner, TaskCore, TaskProgress, Work,
};
use crate::setup::address::StoreAddress;
use crate::setup::touch::{TouchClock, TouchExpectation};
use crate::setup::{SetupError, SetupEvents, SetupFlow, SetupState, SetupWriteReadiness};
use crate::ssh::CardSigner;
use crate::store::PassStore;
use crate::sync::{Cloned, Connection};
use crate::{KeyTouch, PawError};
use crate::sync::hosts::pin_for;
use crate::sync::operation::{self, SyncReport};
use crate::sync::queue::{self, QueueItem};
use crate::sync::{IntegrateReport, RefusedPath, SyncError, accept_replaced_remote};

/// Told how a download is going.
#[uniffi::export(with_foreign)]
pub trait ProgressListener: Send + Sync {
    /// Objects received so far and expected in all. Return `false` to stop.
    fn progress(&self, received: u32, total: u32) -> bool;
}

struct Events {
    connection: Arc<dyn CardConnection>,
    listener: Option<Arc<dyn ProgressListener>>,
}

impl SetupEvents for Events {
    fn touch_needed(&self, _expectation: TouchExpectation) {
        // The state returned before this call says whether it is "needed" or
        // "may be needed", for the wording.
        self.connection.touch_needed();
    }

    fn progress(&self, received: u32, total: u32) -> bool {
        self.listener.as_ref().is_none_or(|l| l.progress(received, total))
    }
}

/// First-run setup; see [`crate::setup`]. Each call is one step and returns
/// the state to show next.
#[derive(uniffi::Object)]
pub struct Setup {
    flow: Mutex<SetupFlow>,
    /// Why the last key task setup started failed, if for a setup reason.
    last_error: Mutex<Option<SetupError>>,
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
                        let error = flow.connection_error(&remote, e);
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
        let mut progress = |received: u32, total: u32| {
            task.set(KeyTaskState::Working { progress: Some(TaskProgress { done: received, total }) });
            !task.cancelled()
        };
        let cloned = crate::sync::clone_with_progress(&connection, &remote, &repo, Some(&mut progress));
        drop(connection);
        if !task.begin_commit() {
            let _ = self.flow().abandon_download();
            return Err(cancelled());
        }
        let cloned: Result<Cloned, PawError> = cloned;
        self.flow().finish_download(&remote, cloned).map_err(|e| self.record(e))?;
        Ok(Some(KeyOutcome::SignedIn))
    }
}

#[uniffi::export]
impl Setup {
    /// Opens the setup kept in `state_dir` and resumes where it stopped.
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Result<Arc<Self>, SetupError> {
        let flow = SetupFlow::open(&PathBuf::from(state_dir))?;
        Ok(Arc::new(Self { flow: Mutex::new(flow), last_error: Mutex::new(None) }))
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

    /// Clones the store: one login, so one signature.
    pub fn download(
        &self,
        connection: Arc<dyn CardConnection>,
        pin: String,
        listener: Arc<dyn ProgressListener>,
    ) -> Result<SetupState, SetupError> {
        let pin = Zeroizing::new(pin);
        let events = Arc::new(Events { connection: connection.clone(), listener: Some(listener) });
        self.flow().download(Arc::new(Adapter(connection)), &pin, events)
    }

    /// Opens one entry to prove the key works.
    pub fn test_decrypt(
        &self,
        connection: Arc<dyn CardConnection>,
        pin: String,
    ) -> Result<SetupState, SetupError> {
        let pin = Zeroizing::new(pin);
        let events = Arc::new(Events { connection: connection.clone(), listener: None });
        self.flow().test_decrypt(Arc::new(Adapter(connection)), &pin, events)
    }

    /// "Your key", from the plugged-in key's session: what the session read
    /// when the key was plugged in. No PIN, no touch, no card exchange.
    pub fn read_key_from_session(&self, session: Arc<KeySession>) -> Result<SetupState, SetupError> {
        let ssh = session.ssh_key().ok().cloned();
        self.flow().read_key_from(session.card_info(), ssh.as_ref())
    }

    /// "Download your store" as a key task (sign-in, `Clone`): the PIN
    /// stage unless the session holds it, `Working` while connecting,
    /// `Touch` while the key signs the login, `Working` with progress while
    /// cloning, then `Done`; read `state` for the next step. A key other
    /// than the one read at "Your key" is refused before the PIN.
    pub fn download_with_key(self: Arc<Self>, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        *lock(&self.last_error) = None;
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

    fn with_card(
        &self,
        connection: Arc<dyn CardConnection>,
        pin: String,
        run: fn(&crate::sync::Remote, &dyn crate::ssh::SshSigner, &std::path::Path) -> Result<SyncReport, SyncError>,
    ) -> Result<SyncReport, SyncError> {
        let pin = Zeroizing::new(pin);
        let remote = self.remote()?;
        let transport = Arc::new(Adapter(connection.clone()));
        let policy = crate::card_info(transport.clone())?.authentication_touch;
        let clock = self.clock(policy);
        let signer = CardSigner::new(transport, &pin, move || {
            if clock.expectation() != TouchExpectation::NotNeeded {
                connection.touch_needed();
            }
        })?;
        drop(pin);
        let report = run(&remote, &signer, &self.repo());
        if report.as_ref().is_ok_and(|r| r.signatures > 0) {
            *self.last_login.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
        }
        report
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
            let result = operation::run_until(&remote, &signer, &sync.repo(), push, &stop);
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

    /// Settings › Store › Forget server key. The next setup or sync of a
    /// host that is not bundled asks to confirm its key again.
    pub fn forget_host_key(&self) -> Result<(), SyncError> {
        let known = crate::setup::known_hosts(&self.state_dir);
        Ok(known.forget(self.address.connect_host(), self.address.port)?)
    }

    /// Whether the next login will wait for a touch, given the slot's policy.
    pub fn touch_expectation(&self, policy: KeyTouch) -> TouchExpectation {
        self.clock(policy).expectation()
    }

    /// "Send": fetch, integrate and push on one login.
    pub fn send(&self, connection: Arc<dyn CardConnection>, pin: String) -> Result<SyncReport, SyncError> {
        self.with_card(connection, pin, operation::send)
    }

    /// Fetch and integrate on one login, pushing nothing.
    pub fn refresh(&self, connection: Arc<dyn CardConnection>, pin: String) -> Result<SyncReport, SyncError> {
        self.with_card(connection, pin, operation::refresh)
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

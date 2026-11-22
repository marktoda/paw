//! Setup and sync as Swift sees them: the setup screens drive [`Setup`];
//! the sync footer, the attention row and the "not yet pushed" screen use
//! [`StoreSync`]. Every card exchange goes through [`CardConnection`], and
//! its `touch_needed` is called only when a touch is, or may be, needed.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use zeroize::Zeroizing;

use super::{Adapter, CardConnection};
use crate::KeyTouch;
use crate::setup::address::StoreAddress;
use crate::setup::touch::{TouchClock, TouchExpectation};
use crate::setup::{SetupError, SetupEvents, SetupFlow, SetupState};
use crate::ssh::CardSigner;
use crate::sync::hosts::pin_for;
use crate::sync::operation::{self, SyncReport};
use crate::sync::queue::{self, QueueItem};
use crate::sync::{IntegrateReport, SyncError, accept_replaced_remote};

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
}

impl Setup {
    fn flow(&self) -> MutexGuard<'_, SetupFlow> {
        // A panic in a step cannot leave the saved state half written: it is
        // written whole, by rename.
        self.flow.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[uniffi::export]
impl Setup {
    /// Opens the setup kept in `state_dir` and resumes where it stopped.
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Result<Arc<Self>, SetupError> {
        let flow = SetupFlow::open(&PathBuf::from(state_dir))?;
        Ok(Arc::new(Self { flow: Mutex::new(flow) }))
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
}

#[uniffi::export]
impl StoreSync {
    /// Opens the store set up in `state_dir`.
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Result<Arc<Self>, SyncError> {
        let state_dir = PathBuf::from(state_dir);
        let address = crate::setup::saved_address(&state_dir)?;
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

    /// After `RemoteReplaced`: take the remote's history, keeping or
    /// discarding this device's unpushed changes.
    pub fn accept_replaced_remote(&self, keep_changes: bool) -> Result<IntegrateReport, SyncError> {
        accept_replaced_remote(&self.repo(), keep_changes)
    }
}

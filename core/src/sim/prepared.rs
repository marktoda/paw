//! A store set up from a [`SimRemote`] the way a phone sets one up, for the
//! app's test store in the simulator: setup's steps run to the end with a
//! simulated key, which pins the key's public half and accepts every
//! `.gpg-id`, so saving then needs no key and no network.
//!
//! The key tasks run as the app runs them, on a key port of their own, and
//! the PIN is typed when one asks. The key must need no touch.

use std::sync::Arc;
use std::time::Duration;

use super::SimulatedKey;
use super::copies::SimClock;
use super::remote::SimRemote;
use super::tasks::{SimDriver, ending, wait_final};
use crate::PawError;
use crate::setup::Setup;
use crate::sync::StoreSync;
use crate::key::KeyPort;
use crate::setup::SetupState;
use crate::task::{KeyTask, KeyTaskListener};

/// The longest one key task may take.
const WITHIN: Duration = Duration::from_secs(60);

fn failed(what: &str, error: impl std::fmt::Display) -> PawError {
    PawError::Store(format!("{what} failed: {error}."))
}

/// Runs the key task `start` makes to its end, typing `pin` when it asks,
/// and stopping if the key refuses it.
fn run(what: &str, pin: &str, start: impl FnOnce(Arc<dyn KeyTaskListener>) -> Arc<KeyTask>) -> Result<(), PawError> {
    let driver = SimDriver::typist_once(pin);
    let task = start(driver.clone());
    driver.drive(&task);
    let Some(end) = wait_final(&task, WITHIN) else {
        task.cancel();
        return Err(PawError::Store(format!("{what} did not finish.")));
    };
    ending(end).map(drop).map_err(|failure| failed(what, failure))
}

/// A key port of its own with `key` plugged in, as the app has one.
fn port_with(key: &Arc<SimulatedKey>) -> Result<Arc<KeyPort>, PawError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
    let port = KeyPort::new(SimClock::new(now));
    port.plugged_in(key.clone()).map_err(|e| failed("Reading the key", e))?;
    Ok(port)
}

#[uniffi::export]
impl SimRemote {
    /// Sets up the store this host serves in `state_dir` as setup does on
    /// a phone: reads `key` (which must need no touch), checks and trusts
    /// the host, downloads with the key and opens one entry, typing `pin`.
    pub fn set_up_store(&self, key: Arc<SimulatedKey>, pin: String, state_dir: String) -> Result<(), PawError> {
        let port = port_with(&key)?;
        let session = port.session().ok_or_else(|| PawError::Store("The key was not read.".into()))?;
        let setup = Setup::open(state_dir).map_err(|e| failed("Setup", e))?;
        setup.read_key_from_session(session).map_err(|e| failed("Reading the key", e))?;
        setup.set_address(self.address()).map_err(|e| failed("The address", e))?;
        if let SetupState::ConfirmHost { fingerprint, .. } =
            setup.check_host().map_err(|e| failed("Checking the host", e))?
        {
            setup.confirm_host(fingerprint).map_err(|e| failed("Trusting the host", e))?;
        }
        run("The download", &pin, |listener| setup.clone().download_with_key(port.clone(), listener))?;
        run("The key check", &pin, |listener| setup.clone().test_decrypt_with_key(port.clone(), listener))?;
        match setup.state() {
            SetupState::Done { .. } => Ok(()),
            state => Err(PawError::Store(format!("Setup stopped at {state:?}."))),
        }
    }

    /// Another computer gives `work/` its own `.gpg-id` naming `key` (and,
    /// with `missing_key`, a second key paw has no copy of), and the store
    /// set up in `state_dir` downloads it with `key`, typing `pin`.
    pub fn give_work_its_own_keys(
        &self,
        key: Arc<SimulatedKey>,
        pin: String,
        state_dir: String,
        missing_key: bool,
    ) -> Result<(), PawError> {
        let mut lines = key.fingerprint_hex() + "\n";
        if missing_key {
            lines += &(SimulatedKey::new_other(pin.clone(), false, 1_000).fingerprint_hex() + "\n");
        }
        self.commit_file("work/.gpg-id".into(), lines.into_bytes(), "Give work its own list of keys.".into())?;
        let sync = StoreSync::open(state_dir).map_err(|e| failed("Opening the store", e))?;
        let port = port_with(&key)?;
        run("The refresh", &pin, |listener| sync.refresh_with_key(port.clone(), listener))
    }
}

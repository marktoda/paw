//! The sync footer, the attention row, the "not yet pushed" screen and
//! Settings › Store's server keys, as Swift sees them.
//!
//! The steps that need the key run as a [`KeyTask`] on the [`KeyPort`]'s
//! session (the `*_with_key` calls): the key sheet binds to the task, the
//! PIN is asked at most once while the key stays plugged in, `Touch` and
//! progress are the task's stages, and `cancel` stops the work between
//! network steps. No call here takes a PIN. A task that fails ends in the
//! state that says what failed: `Failed` for the key, `SyncFailed` for the
//! work after it.

use std::path::PathBuf;
use std::sync::Arc;

use super::address::{StoreAddress, saved_address};
use super::hosts::{KnownHosts, ServerKeys};
use super::queue::{self, QueueItem};
use super::{IntegrateReport, ServerKey, SyncError, accept_replaced_remote};
use crate::files::layout::StoreDirs;
use crate::key::KeyPort;
use crate::remote::{RefusedPath, RemoteFailure};
use crate::task::workflows;
use crate::task::{KeyTask, KeyTaskListener, LoginPurpose};
use crate::write::held_back::{self, HeldBack, ReencryptCheck};

/// Keeping a set-up store in step with its remote.
#[derive(uniffi::Object)]
pub struct StoreSync {
    state_dir: PathBuf,
    address: StoreAddress,
}

impl StoreSync {
    fn store(&self) -> StoreDirs {
        StoreDirs::new(&self.state_dir)
    }

    fn known_hosts(&self) -> KnownHosts {
        KnownHosts::of(&self.state_dir)
    }

    fn with_key(&self, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>, login: LoginPurpose) -> Arc<KeyTask> {
        let (state_dir, address) = (self.state_dir.clone(), self.address.clone());
        workflows::sync(&port, listener, login, state_dir, address)
    }
}

#[uniffi::export]
impl StoreSync {
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Result<Arc<Self>, SyncError> {
        let state_dir = PathBuf::from(state_dir);
        let address = saved_address(&state_dir)?;
        Ok(Arc::new(Self { state_dir, address }))
    }

    pub fn address(&self) -> String {
        self.address.display()
    }

    /// Settings › Store › server keys: the host, whether its keys ship
    /// with paw (and so cannot be forgotten), and each pinned key's type
    /// and fingerprint. A forgotten key leaves `keys` empty until the next
    /// connection is confirmed.
    pub fn server_keys(&self) -> Result<ServerKeys, SyncError> {
        Ok(self.known_hosts().server_keys(&self.address)?)
    }

    /// After Forget: connects to the store's server, without the key and
    /// without logging in, and returns the key it presents, for the person
    /// to compare before [`Self::trust_server_key`]. Network only.
    pub fn check_server_key(&self) -> Result<ServerKey, SyncError> {
        Ok(super::scan_host_key(self.address.connect_host(), self.address.port)?.described())
    }

    /// The person compared `fingerprint` and trusts it: connects again and
    /// pins the key the server presents, if it is the one with that
    /// fingerprint. Refused for a host whose keys ship with paw, and for a
    /// host that already has a saved key (forget it first).
    pub fn trust_server_key(&self, fingerprint: String) -> Result<(), SyncError> {
        let scan = || super::scan_host_key(self.address.connect_host(), self.address.port);
        if !self.known_hosts().trust(&self.address, scan, &fingerprint)? {
            return Err(RemoteFailure::HostKeyNotConfirmed { host: self.address.connect_host().to_owned() }.into());
        }
        Ok(())
    }

    /// Settings › Store › Forget server key. The next setup or sync of a
    /// host that is not bundled asks to confirm its key again. A bundled
    /// host's keys (github.com's, ...) cannot be forgotten:
    /// `RemoteFailure::HostKeyBundled`.
    pub fn forget_host_key(&self) -> Result<(), SyncError> {
        Ok(self.known_hosts().forget(&self.address)?)
    }

    /// "Send" as a key task (sign-in, `Push`): fetch, integrate and push on
    /// one login, with the session's PIN. Ends `Done` with `Synced` and the
    /// report, or `SyncFailed`.
    pub fn send_with_key(self: Arc<Self>, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        self.with_key(port, listener, LoginPurpose::Push)
    }

    /// Fetch and integrate as a key task (sign-in, `Fetch`), pushing
    /// nothing. Ends as `send_with_key` does.
    pub fn refresh_with_key(self: Arc<Self>, port: Arc<KeyPort>, listener: Arc<dyn KeyTaskListener>) -> Arc<KeyTask> {
        self.with_key(port, listener, LoginPurpose::Fetch)
    }

    /// The "not yet pushed" list, newest first.
    pub fn unpushed(&self) -> Result<Vec<QueueItem>, SyncError> {
        queue::unpushed(&self.store())
    }

    /// The waiting changes "Send" will not push, because their ciphertext
    /// is not encrypted for exactly the recipients their entry has now (its
    /// `.gpg-id` changed, or the change is not accepted yet). While any is
    /// listed, a send fetches and integrates but pushes nothing and ends
    /// `Waiting`. Show each entry with who it is encrypted for and who it
    /// should be for; the person accepts the new recipients, then
    /// re-encrypts them with `reencrypt_with_key`, or discards one outright.
    pub fn held_back(&self) -> Result<Vec<HeldBack>, SyncError> {
        held_back::held_back(&self.store())
    }

    /// How many held-back changes "Re-encrypt" can do now (their folder's
    /// new list is accepted and paw has every key on it), and why each
    /// other one can't be done. No key, no network.
    pub fn reencrypt_check(&self) -> Result<ReencryptCheck, SyncError> {
        held_back::reencrypt_check(&self.store())
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
        workflows::reencrypt(&port, listener, self.state_dir.clone())
    }

    pub fn discard(&self, id: String) -> Result<Vec<QueueItem>, SyncError> {
        queue::discard(&self.store(), &id)
    }

    pub fn undo_last(&self) -> Result<Vec<QueueItem>, SyncError> {
        queue::undo_last(&self.store())
    }

    pub fn discard_all(&self) -> Result<(), SyncError> {
        queue::discard_all(&self.store())
    }

    /// What the fetched remote holds that paw will not check out (links,
    /// submodules, unsafe names, names that are the same on an iPhone),
    /// with why; empty if nothing. A sync that fails with
    /// `RemoteFailure::RefusedContent` names the same paths.
    pub fn refused_remote(&self) -> Result<Vec<RefusedPath>, SyncError> {
        Ok(super::refused_remote(&self.store())?)
    }

    /// After `RemoteReplaced`: take the remote's history, keeping or
    /// discarding this device's unpushed changes.
    pub fn accept_replaced_remote(&self, keep_changes: bool) -> Result<IntegrateReport, SyncError> {
        accept_replaced_remote(&self.store(), keep_changes)
    }
}

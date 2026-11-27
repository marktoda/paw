//! The workflows whose work after the key is someone else's: setup's
//! download and what follows its key check, a send or refresh, and
//! re-encrypting held-back changes. Each is one of the key tasks of
//! [`super::start`] with its work after the key.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::{After, DecryptWork, KeyOutcome, KeyPurpose, KeyTask, KeyTaskListener, LoginPurpose, TaskFailure, TaskProgress, Work};
use crate::key::{Halt, KeyIdentity, KeyPort};
use crate::files::layout::StoreDirs;
use crate::locked;
use crate::remote::RemoteFailure;
use crate::setup::{SetupError, SetupFlow};
use crate::sync::address::StoreAddress;
use crate::sync::hosts::KnownHosts;
use crate::sync::{ConnectError, Connection, SyncError, TransferProgress, operation};
use crate::write::held_back::{self, HeldBack, OpenFailure};

/// "Download your store": the clone, on the task's sign-in. The setup is
/// locked only between network steps, so its state answers while they run.
pub(crate) fn download(
    port: &Arc<KeyPort>,
    flow: Arc<Mutex<SetupFlow>>,
    listener: Arc<dyn KeyTaskListener>,
) -> Arc<KeyTask> {
    let expected = locked(&flow).expected_key();
    let work: Work = Box::new(move |job, signer| {
        let (remote, store) = locked(&flow).begin_download()?;
        let connection = match Connection::open(&remote, &signer) {
            Ok(connection) => connection,
            Err(error) => {
                locked(&flow).abandon_download();
                return Err(match error {
                    ConnectError::Key(error) => error.into(),
                    ConnectError::Cancelled => TaskFailure::Cancelled,
                    ConnectError::Fault(fault) => SetupError::from(fault).into(),
                });
            }
        };
        let mut progress = |update: TransferProgress| job.progress(update.into());
        let cloned = crate::sync::clone_with_transfer(&connection, &remote, &store, Some(&mut progress));
        drop(connection);
        if let Err(cancelled) = job.commit() {
            locked(&flow).abandon_download();
            return Err(cancelled);
        }
        locked(&flow).finish_download(cloned)?;
        Ok(KeyOutcome::SignedIn)
    });
    port.sign_in_then(LoginPurpose::Clone, listener, expected, work)
}

/// "Check your key": opens the entry setup picked, then finishes setup
/// and gives the key to `port` as the enrolled one.
pub(crate) fn check_key(
    port: &Arc<KeyPort>,
    flow: Arc<Mutex<SetupFlow>>,
    listener: Arc<dyn KeyTaskListener>,
) -> Arc<KeyTask> {
    let (expected, found) = {
        let flow = locked(&flow);
        (flow.expected_key(), flow.pick_test_entry().and_then(|entry| Ok((entry, flow.open_store()?))))
    };
    let (entry, store) = match found {
        Ok(found) => found,
        Err(error) => return KeyTask::refused(KeyPurpose::CheckKey { entry: None }, listener, error.into()),
    };
    let enrol = port.clone();
    let opened = entry.clone();
    let after: After = Box::new(move |session| {
        let key = session.decryption_key()?;
        let identity = KeyIdentity::of(session.card_info());
        locked(&flow).finish_test_decrypt(opened, key, &identity)?;
        enrol.set_enrolled_key(Some(identity));
        Ok(())
    });
    port.check_key_then(store, entry, listener, expected, after)
}

/// A send (`login` is `Push`) or a refresh of the set-up store in
/// `state_dir`, on one login.
pub(crate) fn sync(
    port: &Arc<KeyPort>,
    listener: Arc<dyn KeyTaskListener>,
    login: LoginPurpose,
    state_dir: PathBuf,
    address: StoreAddress,
) -> Arc<KeyTask> {
    let push = matches!(login, LoginPurpose::Push);
    let work: Work = Box::new(move |job, signer| {
        let remote = KnownHosts::of(&state_dir)
            .remote(&address)
            .map_err(SyncError::from)?
            .ok_or_else(|| SyncError::from(RemoteFailure::NoSavedHostKey { host: address.connect_host().to_owned() }))?;
        let store = StoreDirs::new(&state_dir);
        let holds_back = || held_back::holds_back(&store);
        let stop = || job.cancelled();
        let mut progress = |update: TransferProgress| job.progress(update.into());
        let report = operation::run_until::<TaskFailure>(&remote, &signer, &store, &holds_back, push, &stop, Some(&mut progress))?;
        Ok(KeyOutcome::Synced { report })
    });
    let expected = port.expected();
    port.sign_in_then(login, listener, expected, work)
}

/// "Re-encrypt": opens each held-back change of the store in `state_dir`
/// that is ready and encrypts it for its folder's new list, one touch each.
pub(crate) fn reencrypt(
    port: &Arc<KeyPort>,
    listener: Arc<dyn KeyTaskListener>,
    state_dir: PathBuf,
) -> Arc<KeyTask> {
    let store = StoreDirs::new(&state_dir);
    if let Err(error) = held_back::reencrypt_check(&store) {
        return KeyTask::refused(KeyPurpose::Reencrypt, listener, error.into());
    }
    let work: DecryptWork = Box::new(move |job, opener| {
        let mut open = |_: &HeldBack, ciphertext: &[u8]| {
            opener(ciphertext).map_err(|halt| match halt {
                Halt::Fail(error) => OpenFailure::<TaskFailure>::from(error),
                halt => OpenFailure::Stop(halt.into()),
            })
        };
        let mut progress = |done: u32, total: u32| {
            job.progress(TaskProgress { done, total, bytes: None });
        };
        let report = held_back::reencrypt_each(&store, &mut open, &mut progress)?;
        Ok(KeyOutcome::Reencrypted { report })
    });
    port.decrypt_then(listener, work)
}

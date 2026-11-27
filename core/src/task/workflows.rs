//! The workflows whose work after the key is someone else's: setup's
//! download and what follows its key check, a send or refresh, and
//! re-encrypting held-back changes. Each is one of the key tasks of
//! [`super::start`] with its work after the key; the runner decides how
//! every one of them ends once cancelled.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{After, DecryptWork, KeyOutcome, KeyPurpose, KeyTask, KeyTaskListener, LoginPurpose, TaskFailure, TaskProgress, Work};
use crate::key::{Halt, KeyIdentity, KeyPort};
use crate::locked;
use crate::remote::RemoteFailure;
use crate::setup::{SetupError, SetupFlow};
use crate::store::PassStore;
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
        let (remote, repo) = locked(&flow).begin_download()?;
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
        locked(&flow).note_login();
        let mut progress = |update: TransferProgress| job.progress(update.into());
        let cloned = crate::sync::clone_with_transfer(&connection, &remote, &repo, Some(&mut progress));
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
    let (expected, picked, repo) = {
        let flow = locked(&flow);
        (flow.expected_key(), flow.pick_test_entry(), flow.repo_dir())
    };
    let found = picked.and_then(|entry| Ok((entry, PassStore::open(repo.to_string_lossy().into_owned())?)));
    let (entry, store) = match found {
        Ok(found) => found,
        Err(error) => return KeyTask::failed(KeyPurpose::CheckKey { entry: None }, listener, error.into()),
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
/// `state_dir`, on one login. `last_login` is when the authentication
/// slot last signed, for a cached touch policy.
pub(crate) fn sync(
    port: &Arc<KeyPort>,
    listener: Arc<dyn KeyTaskListener>,
    login: LoginPurpose,
    state_dir: PathBuf,
    address: StoreAddress,
    last_login: Arc<Mutex<Option<Instant>>>,
) -> Arc<KeyTask> {
    let push = matches!(login, LoginPurpose::Push { .. });
    let work: Work = Box::new(move |job, signer| {
        let remote = KnownHosts::of(&state_dir)
            .remote(&address)
            .map_err(SyncError::from)?
            .ok_or_else(|| SyncError::from(RemoteFailure::NoSavedHostKey { host: address.connect_host().to_owned() }))?;
        let repo = crate::files::layout::repo_dir(&state_dir);
        let holds_back = || held_back::holds_back(&repo, &state_dir);
        let stop = || job.cancelled();
        let mut progress = |update: TransferProgress| job.progress(update.into());
        let report = operation::run_until::<TaskFailure>(&remote, &signer, &repo, &holds_back, push, &stop, Some(&mut progress))?;
        if report.signatures > 0 {
            *locked(&last_login) = Some(Instant::now());
        }
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
    let repo = crate::files::layout::repo_dir(&state_dir);
    let count = match held_back::reencrypt_check(&repo, &state_dir) {
        Ok(check) => check.ready,
        Err(error) => return KeyTask::failed(KeyPurpose::Reencrypt { count: 0 }, listener, error.into()),
    };
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
        let report = held_back::reencrypt_each(&repo, &state_dir, &mut open, &mut progress)?;
        Ok(KeyOutcome::Reencrypted { report })
    });
    port.decrypt_then(KeyPurpose::Reencrypt { count }, listener, work)
}

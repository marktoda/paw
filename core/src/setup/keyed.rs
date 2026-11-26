//! The setup steps that run in a key task: the download, on the task's
//! sign-in, and what follows the key check. The setup is locked only
//! between network steps, so its state answers while they run.

use std::sync::{Arc, Mutex, MutexGuard};

use super::{SetupError, SetupFlow};
use crate::PawError;
use crate::key::{Failure, KeyError, KeyIdentity, KeyPort, KeySession, SessionSigner, TaskSide};
use crate::store::PassStore;
use crate::sync::{Cloned, ConnectError, Connection, TransferProgress};

/// How a keyed setup step failed: the key, or setup.
pub(crate) type StepFailure = Failure<SetupError>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Plain values; a panic cannot leave them half written: the saved state
    // is written whole, by rename.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The download, in a sign-in task's thread once the PIN is verified: the
/// clone's progress goes to `transfer` and to the task.
pub(crate) fn download(
    flow: &Mutex<SetupFlow>,
    transfer: &Mutex<Option<TransferProgress>>,
    task: &dyn TaskSide,
    signer: &SessionSigner,
) -> Result<(), StepFailure> {
    let cancelled = || Failure::Key(KeyError::Cancelled);
    let (remote, repo) = lock(flow).begin_download().map_err(Failure::Work)?;
    if task.cancelled() {
        let _ = lock(flow).abandon_download();
        return Err(cancelled());
    }
    let connection = match Connection::open(&remote, signer) {
        Ok(connection) => connection,
        Err(error) => {
            let mut flow = lock(flow);
            let _ = flow.abandon_download();
            if task.cancelled() {
                return Err(cancelled());
            }
            return Err(match error {
                ConnectError::Key(error) => error.into(),
                ConnectError::Remote(error) => Failure::Work(flow.connection_error(&remote, error, None)),
            });
        }
    };
    lock(flow).note_login();
    let mut progress = |update: TransferProgress| {
        if task.cancelled() {
            return false;
        }
        *lock(transfer) = Some(update);
        task.report_progress(update.received_objects, update.total_objects)
    };
    let cloned = crate::sync::clone_with_transfer(&connection, &remote, &repo, Some(&mut progress));
    let said = connection.remote_said();
    drop(connection);
    if !task.begin_commit() {
        let _ = lock(flow).abandon_download();
        return Err(cancelled());
    }
    let cloned: Result<Cloned, PawError> = cloned;
    lock(flow).finish_download(&remote, cloned, said).map_err(Failure::Work)?;
    Ok(())
}

/// The entry the key check opens, and the downloaded store it is in.
pub(crate) fn test_entry(flow: &Mutex<SetupFlow>) -> Result<(String, Arc<PassStore>), SetupError> {
    let (picked, repo) = {
        let flow = lock(flow);
        (flow.pick_test_entry(), flow.repo_dir())
    };
    let entry = picked?;
    let store = PassStore::open(repo.to_string_lossy().into_owned())?;
    Ok((entry, store))
}

/// After the key opened `entry`: pins the card's key for saving, accepts
/// the store's recipients, and records the key as the store's enrolled key,
/// on disk and in `port`. Setup is then done.
pub(crate) fn after_check(
    flow: &Mutex<SetupFlow>,
    port: &KeyPort,
    session: &KeySession,
    entry: String,
) -> Result<(), StepFailure> {
    let key = session.decryption_key()?;
    let identity = KeyIdentity::of(session.card_info());
    lock(flow).finish_test_decrypt(entry, key, &identity).map_err(Failure::Work)?;
    port.set_enrolled_key(Some(identity));
    Ok(())
}

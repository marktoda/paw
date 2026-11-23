//! An [`SshSigner`] that signs through the key session, so that clone,
//! fetch, push and setup share the PIN the person typed once.
//!
//! It comes from a sign-in task ([`KeyTask::login_signer`]), which has
//! already verified the PIN before the connection opens. Each signature is
//! one card job: the held PIN is re-verified silently if the card dropped
//! it, then INTERNAL AUTHENTICATE runs, showing `Touch` on the task as the
//! slot's policy says. No PIN is asked while the server waits: if paw no
//! longer holds one, the signature fails.
//!
//! [`KeyTask::login_signer`]: super::KeyTask::login_signer

use std::sync::Arc;

use super::session::{CardJob, Halt, KeySession, card_error};
use super::status::{self, KeyOperation};
use super::task::TaskCore;
use super::touch::{self, TouchWatch};
use super::{KeyError, KeyTaskState};
use crate::PawError;
use crate::ssh::{SshPublicKey, SshSigner};

const INTERNAL_AUTHENTICATE: u8 = 0x88;

pub struct SessionSigner {
    task: Arc<TaskCore>,
    session: Arc<KeySession>,
    public_key: SshPublicKey,
}

impl SessionSigner {
    pub(crate) fn new(task: Arc<TaskCore>, session: Arc<KeySession>) -> Result<Self, KeyError> {
        let public_key = session.ssh_key()?.clone();
        Ok(Self { task, session, public_key })
    }
}

/// The SSH code speaks [`PawError`]; the task keeps the precise error.
fn paw_error(error: &KeyError) -> PawError {
    match error {
        KeyError::WrongPin { tries_left } | KeyError::HeldPinRejected { tries_left } => {
            PawError::WrongPin { tries_left: *tries_left }
        }
        KeyError::PinBlocked => PawError::PinBlocked,
        KeyError::NoAuthenticationKey => PawError::NoSshKey(error.to_string()),
        other => PawError::Sign(other.to_string()),
    }
}

fn authenticate(job: &mut CardJob<'_>, task: &Arc<TaskCore>, data: &[u8]) -> Result<Vec<u8>, Halt> {
    let session = job.session();
    let wire = session.wire().clone();
    let policy = session.card_info().authentication_touch;
    let mut tx = job.card.transaction().map_err(|e| card_error(&wire, "starting to sign", e))?;
    let _watch = TouchWatch::arm(&wire, task, policy);
    wire.clear_last();
    let error = match tx.card().internal_authenticate(data.to_vec()) {
        Ok(signature) if signature.len() == 64 => return Ok(signature),
        Ok(signature) => {
            return Err(Halt::Fail(KeyError::Reader {
                detail: format!("expected a 64-byte signature, the key returned {} bytes", signature.len()),
            }));
        }
        Err(e) => e,
    };
    if wire.failed() {
        return Err(Halt::Fail(KeyError::KeyRemoved));
    }
    Err(Halt::Fail(match wire.last() {
        Some((INTERNAL_AUTHENTICATE, sw)) => match status::key_operation(sw, touch::possible(policy)) {
            KeyOperation::TouchTimeout => KeyError::TouchTimeout,
            KeyOperation::Refused(sw) => status::unexpected("signing", sw),
            KeyOperation::Done => KeyError::Reader { detail: format!("signing: {error}") },
        },
        _ => card_error(&wire, "signing", error),
    }))
}

impl SshSigner for SessionSigner {
    fn public_key(&self) -> &SshPublicKey {
        &self.public_key
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, PawError> {
        let cancelled = || PawError::Sign(KeyError::Cancelled.to_string());
        if self.task.cancelled() {
            return Err(cancelled());
        }
        let result = self.session.run(&self.task, |job| {
            job.unlock(None)?;
            authenticate(job, &self.task, data)
        });
        let error = match result {
            Ok(_) if self.task.cancelled() => return Err(cancelled()),
            Ok(signature) => {
                self.task.set(KeyTaskState::Working { progress: None });
                return Ok(signature);
            }
            Err(Halt::AskPin(Some(error)) | Halt::Fail(error)) => error,
            Err(Halt::AskPin(None)) => KeyError::PinForgotten,
        };
        let paw = paw_error(&error);
        self.task.set(KeyTaskState::Failed { error });
        Err(paw)
    }
}

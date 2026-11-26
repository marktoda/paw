//! What signs an SSH login ([`SshSigner`]), and the signer that signs
//! through the key session, so that clone, fetch, push and setup share the
//! PIN the person typed once.
//!
//! No PIN is asked while the server waits: if paw no longer holds one, the
//! signature fails.

use std::sync::Arc;
use std::time::Instant;

use super::session::{CardJob, Halt, KeySession, card_error};
use super::apdu;
use super::task::TaskSide;
use super::touch::{self, TouchWatch};
use super::KeyError;
use crate::ssh::SshPublicKey;

/// Something that can sign an SSH login: the card today, a device key later.
pub trait SshSigner: Send + Sync {
    fn public_key(&self) -> &SshPublicKey;

    /// Signs `data` and returns the bare signature (64 bytes for ed25519).
    /// May block for as long as the key waits for a touch.
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, KeyError>;
}

pub struct SessionSigner {
    task: Arc<dyn TaskSide>,
    session: Arc<KeySession>,
    public_key: SshPublicKey,
}

impl SessionSigner {
    pub(crate) fn new(task: Arc<dyn TaskSide>, session: Arc<KeySession>) -> Result<Self, KeyError> {
        let public_key = session.ssh_key()?.clone();
        Ok(Self { task, session, public_key })
    }
}

pub(crate) fn authenticate(job: &mut CardJob<'_>, task: &Arc<dyn TaskSide>, data: &[u8]) -> Result<Vec<u8>, Halt> {
    let session = job.session();
    let wire = session.wire().clone();
    let policy = session.card_info().authentication_touch;
    let mut tx = job.card.transaction().map_err(|e| card_error(&wire, "starting to sign", e))?;
    let _watch = TouchWatch::arm(&wire, task, policy, "authentication");
    wire.clear_last();
    let sent = Instant::now();
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
    match wire.last() {
        // A lost PIN cannot be asked for while the server waits: the
        // signature fails with `PinForgotten` (see `sign`).
        Some((apdu::INTERNAL_AUTHENTICATE, sw)) if apdu::security_refusal(sw) => {
            Err(session.refused(task.as_ref(), "signing", sw, touch::possible(policy), sent.elapsed()))
        }
        _ => Err(Halt::Fail(card_error(&wire, "signing", error))),
    }
}

impl SshSigner for SessionSigner {
    fn public_key(&self) -> &SshPublicKey {
        &self.public_key
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, KeyError> {
        if self.task.cancelled() {
            return Err(KeyError::Cancelled);
        }
        let result = self.session.run(self.task.as_ref(), |job| {
            job.unlock(None)?;
            authenticate(job, &self.task, data)
        });
        match result {
            Ok(_) if self.task.cancelled() => Err(KeyError::Cancelled),
            Ok(signature) => {
                self.task.show_working();
                Ok(signature)
            }
            Err(Halt::AskPin(Some(error)) | Halt::Fail(error)) => Err(error),
            Err(Halt::AskPin(None)) => Err(KeyError::PinForgotten),
        }
    }
}

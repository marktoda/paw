//! What signs an SSH login ([`SshSigner`]), and the signer that signs
//! through the key session, so that clone, fetch, push and setup share the
//! PIN the person typed once.
//!
//! No PIN is asked while the server waits: if paw no longer holds one, the
//! signature fails.

use std::sync::Arc;

use super::session::{CardJob, CardStop, Halt, KeySession, card_error};
use super::task::TaskSide;
use super::{KeyError, KeySlot};
use crate::ssh::SshPublicKey;

/// Something that can sign an SSH login: the card today, a device key later.
pub trait SshSigner: Send + Sync {
    fn public_key(&self) -> &SshPublicKey;

    /// Signs `data` and returns the bare signature (64 bytes for ed25519).
    /// May block for as long as the key waits for a touch.
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, SignFailure>;
}

/// Why a signer gave no signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignFailure {
    Key(KeyError),
    /// The task the signer belongs to was cancelled.
    Cancelled,
}

impl From<KeyError> for SignFailure {
    fn from(error: KeyError) -> Self {
        Self::Key(error)
    }
}

pub struct SessionSigner {
    task: Arc<dyn TaskSide>,
    session: Arc<KeySession>,
    public_key: SshPublicKey,
}

impl SessionSigner {
    /// `public_key` is `session`'s authentication key.
    pub(crate) fn new(task: Arc<dyn TaskSide>, session: Arc<KeySession>, public_key: SshPublicKey) -> Self {
        Self { task, session, public_key }
    }
}

pub(crate) fn authenticate(job: &mut CardJob<'_>, task: &Arc<dyn TaskSide>, data: &[u8]) -> Result<Vec<u8>, CardStop> {
    let session = job.session();
    let wire = session.wire().clone();
    let mut tx = job.card.transaction().map_err(|e| card_error(&wire, "starting to sign", e))?;
    // A lost PIN cannot be asked for while the server waits: the signature
    // fails with `PinForgotten` (see `sign`).
    match session.touch_op(task, KeySlot::Authentication, || tx.card().internal_authenticate(data.to_vec()))? {
        Ok(signature) if signature.len() == 64 => Ok(signature),
        Ok(signature) => Err(KeyError::Reader {
            detail: format!("expected a 64-byte signature, the key returned {} bytes", signature.len()),
        }
        .into()),
        Err(error) => Err(KeyError::Reader { detail: format!("signing: {error}") }.into()),
    }
}

impl SshSigner for SessionSigner {
    fn public_key(&self) -> &SshPublicKey {
        &self.public_key
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, SignFailure> {
        if self.task.cancelled() {
            return Err(SignFailure::Cancelled);
        }
        let result = self.session.run(self.task.as_ref(), |job| {
            job.unlock(None)?;
            authenticate(job, &self.task, data)
        });
        match result {
            Ok(_) if self.task.cancelled() => Err(SignFailure::Cancelled),
            Ok(signature) => {
                self.task.show_working();
                Ok(signature)
            }
            Err(CardStop::AskPin(Some(error)) | CardStop::Halt(Halt::Fail(error))) => Err(error.into()),
            Err(CardStop::AskPin(None)) => Err(KeyError::PinForgotten.into()),
            Err(CardStop::Halt(Halt::Cancelled)) => Err(SignFailure::Cancelled),
        }
    }
}

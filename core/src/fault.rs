//! What can go wrong underneath any workflow. Every workflow's error
//! carries a [`Fault`] as one case beside its own, and turns one
//! into itself the same way ([`workflow_error!`]), so a fault keeps its
//! meaning however many layers it passes through. Only [`Fault::Local`]
//! is text: a failure nobody can act on except by trying again.

use crate::PawError;
use crate::remote::{RefusedPath, RemoteFailure};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Enum)]
pub enum Fault {
    /// See [`PawError::StoreBusy`].
    #[error("store busy")]
    StoreBusy,
    /// The remote, its host or the GitHub sign-in that reads it.
    #[error(transparent)]
    Remote { failure: RemoteFailure },
    /// A change would have put these paths in the store, which paw refuses
    /// to commit for the reasons it refuses to check them out. Nothing was
    /// written.
    #[error("refused {paths:?}")]
    Refused { paths: Vec<RefusedPath> },
    /// The store on this device, its state files, git or the keychain
    /// failed. `detail` never holds an entry's contents.
    #[error("{detail}")]
    Local { detail: String },
    /// Stopped between two steps, as asked. Nothing is left half done.
    #[error("cancelled")]
    Cancelled,
}

impl Fault {
    pub(crate) fn local(detail: impl std::fmt::Display) -> Self {
        Self::Local { detail: detail.to_string() }
    }
}

impl From<PawError> for Fault {
    fn from(error: PawError) -> Self {
        match error {
            PawError::StoreBusy => Self::StoreBusy,
            other => Self::local(other),
        }
    }
}

impl From<RemoteFailure> for Fault {
    fn from(failure: RemoteFailure) -> Self {
        Self::Remote { failure }
    }
}

impl crate::remote::AboutRemote for Fault {
    fn remote_failure(&self) -> Option<&RemoteFailure> {
        match self {
            Self::Remote { failure } => Some(failure),
            _ => None,
        }
    }
}

/// Makes `$error` a workflow error: it has a `Fault { fault }` case, and a
/// [`Fault`], a [`PawError`] or a [`RemoteFailure`] turns into it through
/// that case.
macro_rules! workflow_error {
    ($error:ty) => {
        impl From<$crate::fault::Fault> for $error {
            fn from(fault: $crate::fault::Fault) -> Self {
                Self::Fault { fault }
            }
        }

        impl From<$crate::PawError> for $error {
            fn from(error: $crate::PawError) -> Self {
                $crate::fault::Fault::from(error).into()
            }
        }

        impl From<$crate::remote::RemoteFailure> for $error {
            fn from(failure: $crate::remote::RemoteFailure) -> Self {
                $crate::fault::Fault::from(failure).into()
            }
        }

        impl $crate::remote::AboutRemote for $error {
            fn remote_failure(&self) -> Option<&$crate::remote::RemoteFailure> {
                match self {
                    Self::Fault { fault } => $crate::remote::AboutRemote::remote_failure(fault),
                    _ => None,
                }
            }
        }
    };
}
pub(crate) use workflow_error;

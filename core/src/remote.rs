//! Why talking to a store's remote did not work, said once for every
//! workflow that talks to it: setup, sync and the GitHub sign-in each carry
//! a [`RemoteFailure`] instead of listing these cases again.

use crate::sync::RefusedPath;
use crate::sync::hosts::PortFallback;

/// What went wrong with the remote: the connection to its host, the
/// repository there, or the GitHub sign-in that reads it. The store on this
/// device was not changed by any of these.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Enum)]
pub enum RemoteFailure {
    /// No connection was made at all: the TCP connection was refused, found
    /// no route or no such host, or the SSH handshake did not finish in
    /// time. A network that blocks the port looks like this. `fallback` is
    /// another way in to the same host, if it has one.
    #[error("could not reach the git host: {detail}")]
    NoConnection { host: String, detail: String, fallback: Option<PortFallback> },
    /// The network or the host failed once connected.
    #[error("could not reach the git host: {detail}")]
    Unreachable { host: String, detail: String },
    /// The host presented a key other than the pinned ones. Nothing was
    /// sent. `bundled`: its keys ship with paw, so there is nothing to
    /// confirm instead.
    #[error("{host}'s key changed: expected {}, it presented {presented}", .expected.join(" or "))]
    HostKeyChanged { host: String, expected: Vec<String>, presented: String, bundled: bool },
    /// The host presents a key other than the one the person checked.
    /// Nothing was pinned.
    #[error("{host} now shows a different key from the one you checked")]
    HostKeyNotConfirmed { host: String },
    /// No key is pinned for the host (it was forgotten): it must be
    /// confirmed again first.
    #[error("{host} has no saved key; set the store up again")]
    NoSavedHostKey { host: String },
    /// The host's keys ship with paw: they are never forgotten, and no
    /// other key is pinned for it.
    #[error("{host}'s keys come with paw and can't be forgotten")]
    HostKeyBundled { host: String },
    /// The host does not accept the key (`public_key`, as an
    /// `authorized_keys` line). Found before the PIN or a touch.
    #[error("{host} doesn't know this key yet")]
    KeyNotAccepted { host: String, public_key: String },
    /// The key's authentication key can't be offered for an SSH login.
    #[error("this key cannot log in to a git host: {detail}")]
    LoginKeyUnusable { detail: String },
    /// The login went through and the host says the repository is not
    /// there, or that this key can't see it. `said` is what the host
    /// printed (its words, not paw's), `detail` what git reported.
    #[error("{host} says {repository} doesn't exist, or this key can't see it")]
    RepositoryMissing { host: String, repository: String, said: Option<String>, detail: String },
    /// The remote holds something paw will not check out. Nothing was
    /// checked out; it has to be fixed from another computer.
    #[error("{}", crate::sync::guard::refused_message(.paths))]
    RefusedContent { paths: Vec<RefusedPath> },
    /// The host refused the update by a rule of its own. `detail` is what
    /// git and the host said.
    #[error("{detail}")]
    PushRefused { detail: String },
    /// The remote moved again while pushing, after one retry.
    #[error("the remote kept changing; try again")]
    KeptMoving,
    /// No GitHub sign-in is kept on this device. Nothing was sent.
    #[error("not signed in to GitHub")]
    NotSignedIn,
    /// GitHub no longer accepts the sign-in (revoked, or unused for six
    /// months). It has been forgotten here.
    #[error("GitHub sign-in ended; sign in again")]
    SignedOut,
    /// GitHub refused the access token (401), also after it was renewed.
    #[error("GitHub did not accept paw's sign-in")]
    Unauthorised,
    /// No GitHub repository has been chosen for this store yet.
    #[error("no GitHub repository is chosen for this store")]
    NotLinked,
    /// The store's remote is not the repository the sign-in is for.
    #[error("this store is {store}, but GitHub sign-in is for {granted}; paw did not use it")]
    WrongRepository { store: String, granted: String },
    /// GitHub answered something paw does not accept. `message` is paw's
    /// account of it, never a token.
    #[error("GitHub: {message}")]
    GitHubRefused { message: String },
}

/// An error that may say why the remote failed.
pub(crate) trait AboutRemote {
    fn remote_failure(&self) -> Option<&RemoteFailure>;

    fn is_unauthorised(&self) -> bool {
        matches!(self.remote_failure(), Some(RemoteFailure::Unauthorised))
    }
}

//! Why talking to a store's remote did not work, said once for every
//! workflow that talks to it: setup, sync and the GitHub sign-in each carry
//! a [`RemoteFailure`] instead of listing these cases again. The app words
//! them; the texts here are for logs and tests.
//!
//! Also the values a failure carries that other parts of paw share: what
//! paw refuses to check out ([`RefusedPath`]), which the write path refuses
//! to commit for the same reasons, and the other way in to a host
//! ([`PortFallback`]), which setup also keeps as a notice.

use crate::path::UnsafePart;

/// What went wrong with the remote: the connection to its host, the
/// repository there, or the GitHub sign-in that reads it. The store on this
/// device was not changed by any of these.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Enum)]
pub enum RemoteFailure {
    /// No connection was made at all: the TCP connection was refused, found
    /// no route or no such host, or the SSH handshake did not finish in
    /// time. A network that blocks the port looks like this. `fallback` is
    /// another way in to the same host, if it has one.
    #[error("no connection to {host}: {detail}")]
    NoConnection { host: String, detail: String, fallback: Option<PortFallback> },
    /// The network or the host failed once connected.
    #[error("{host} unreachable: {detail}")]
    Unreachable { host: String, detail: String },
    /// The host presented a key other than the pinned ones. Nothing was
    /// sent. `bundled`: its keys ship with paw, so there is nothing to
    /// confirm instead.
    #[error("{host} host key changed: expected {expected:?}, presented {presented}")]
    HostKeyChanged { host: String, expected: Vec<String>, presented: String, bundled: bool },
    /// The host presents a key other than the one the person checked.
    /// Nothing was pinned.
    #[error("{host} host key not the one checked")]
    HostKeyNotConfirmed { host: String },
    /// No key is pinned for the host (it was forgotten): it must be
    /// confirmed again first.
    #[error("{host} has no saved host key")]
    NoSavedHostKey { host: String },
    /// The host's keys ship with paw: they are never forgotten, and no
    /// other key is pinned for it.
    #[error("{host} host keys are bundled")]
    HostKeyBundled { host: String },
    /// The host does not accept the key (`public_key`, as an
    /// `authorized_keys` line). Found before the PIN or a touch.
    #[error("{host} does not accept {public_key}")]
    KeyNotAccepted { host: String, public_key: String },
    /// The key's authentication key can't be offered for an SSH login.
    #[error("login key unusable: {detail}")]
    LoginKeyUnusable { detail: String },
    /// The login went through and the host says the repository is not
    /// there, or that this key can't see it. `said` is what the host
    /// printed (its words, not paw's), `detail` what git reported.
    #[error("{host}: no repository {repository}: {detail}")]
    RepositoryMissing { host: String, repository: String, said: Option<String>, detail: String },
    /// The remote holds something paw will not check out. Nothing was
    /// checked out; it has to be fixed from another computer.
    #[error("refused content: {paths:?}")]
    RefusedContent { paths: Vec<RefusedPath> },
    /// The host refused the update by a rule of its own. `detail` is what
    /// git and the host said.
    #[error("push refused: {detail}")]
    PushRefused { detail: String },
    /// The remote moved again while pushing, after one retry.
    #[error("remote kept moving")]
    KeptMoving,
    /// No GitHub sign-in is kept on this device. Nothing was sent.
    #[error("not signed in to GitHub")]
    NotSignedIn,
    /// GitHub no longer accepts the sign-in (revoked, or unused for six
    /// months). It has been forgotten here.
    #[error("GitHub sign-in ended")]
    SignedOut,
    /// GitHub refused the access token (401), also after it was renewed.
    #[error("GitHub: unauthorised")]
    Unauthorised,
    /// No GitHub repository has been chosen for this store yet.
    #[error("no GitHub repository linked")]
    NotLinked,
    /// The store's remote is not the repository the sign-in is for.
    #[error("store is {store}, sign-in is for {granted}")]
    WrongRepository { store: String, granted: String },
    /// GitHub answered something paw does not accept. `message` is paw's
    /// account of it, never a token.
    #[error("GitHub refused: {message}")]
    GitHubRefused { message: String },
}

/// An error that may say why the remote failed.
pub(crate) trait AboutRemote {
    fn remote_failure(&self) -> Option<&RemoteFailure>;

    fn is_unauthorised(&self) -> bool {
        matches!(self.remote_failure(), Some(RemoteFailure::Unauthorised))
    }
}

/// A path in a repository that paw will not check out, and why.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RefusedPath {
    /// The path as the repository names it (lossy if it is not UTF-8).
    pub path: String,
    /// Why: at least one reason, in the order they were found.
    pub reasons: Vec<RefusedReason>,
}

/// Why paw refuses a path in a tree (`sync::guard::check_tree`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RefusedReason {
    /// The whole tree, at path `/`: it has more than `max_entries` files
    /// and folders, or more than `max_mib` MiB of names.
    TooBig { max_entries: u32, max_mib: u32 },
    NotUtf8,
    HasSlash,
    PathTooLong,
    UnsafeName { part: UnsafePart },
    /// Some file system would take the name for `.gpg-id`.
    GpgIdAlias,
    GpgIdNotFile,
    NestedTooDeep,
    SymbolicLink,
    Submodule,
    NotRegularFile,
    AppearsTwice,
    /// Case, Unicode normalisation or ignored code points make it the
    /// same name as `first` on the phone's file system.
    SameNameAs { first: String },
}

/// Another way in to a host whose usual port a network may block, with the
/// same keys: "Port 22 seems blocked. Try GitHub's port 443?"
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PortFallback {
    /// The host and port that could not be reached, e.g. github.com, 22.
    pub from_host: String,
    pub from_port: u16,
    /// Where to try instead, e.g. ssh.github.com, 443.
    pub host: String,
    pub port: u16,
}

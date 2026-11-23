//! What the git host's answer meant, for the screen after a host check or
//! a download that did not go through.
//!
//! `SetupError` cannot gain a case without breaking Swift's exhaustive
//! switches, so a few answers that need their own screen share an existing
//! case (`Failed` for "no such repository") and are told apart here:
//! [`super::SetupFlow::last_remote_message`] says what the last check or
//! download found.

/// Why the host check or the download stopped, as far as the host's side
/// goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteReason {
    /// The host accepted the key's login but says the repository does not
    /// exist, or that this key's account cannot see it (GitHub: "ERROR:
    /// Repository not found."). Not the same as a key the host does not
    /// know at all.
    RepositoryNotFound,
    /// The host does not accept this key at all. Found before any PIN or
    /// signature.
    KeyNotAccepted,
    /// No connection: no network, no such host, or the port refused or
    /// timed out.
    Unreachable,
    /// The host presented a key other than the one pinned for it.
    HostKeyChanged,
    /// The repository has no commits.
    EmptyRepository,
    /// The repository has no `.gpg-id` file at the top.
    NotAPassStore,
    /// The host or git failed in some other way; see `said`.
    Other,
}

/// What the last host check or download found out about the remote.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteMessage {
    pub reason: RemoteReason,
    /// The host as connected to, e.g. `github.com`.
    pub host: String,
    /// The repository as asked for, e.g. `you/password-store.git`.
    pub repository: String,
    /// What the host itself printed, if anything (control characters
    /// removed, at most 2 KiB). The host's words, not paw's: show them only
    /// as a quote, if at all.
    pub said: Option<String>,
}

impl RemoteReason {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::RepositoryNotFound => "repository-not-found",
            Self::KeyNotAccepted => "key-not-accepted",
            Self::Unreachable => "unreachable",
            Self::HostKeyChanged => "host-key-changed",
            Self::EmptyRepository => "empty-repository",
            Self::NotAPassStore => "not-a-pass-store",
            Self::Other => "other",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        [
            Self::RepositoryNotFound,
            Self::KeyNotAccepted,
            Self::Unreachable,
            Self::HostKeyChanged,
            Self::EmptyRepository,
            Self::NotAPassStore,
            Self::Other,
        ]
        .into_iter()
        .find(|reason| reason.name() == name)
    }
}

/// Whether what a host printed when a git command failed says the
/// repository is not there, or not there for this key. Hosts word it
/// differently:
/// - GitHub: "ERROR: Repository not found."
/// - GitLab: "ERROR: The project you were looking for could not be found or
///   you don't have permission to view it."
/// - Gitea, Forgejo (Codeberg): "... Repository does not exist"
/// - Bitbucket: "repository does not exist", "Forbidden"
/// - git-daemon and plain `git-upload-pack`: "... does not appear to be a
///   git repository"
pub(crate) fn says_repository_missing(said: &str) -> bool {
    let said = said.to_ascii_lowercase();
    if said.contains("command not found") {
        return false;
    }
    [
        "repository not found",
        "could not be found",
        "does not exist",
        "not appear to be a git repository",
        "you don't have permission",
        "access denied",
        "forbidden",
    ]
    .iter()
    .any(|phrase| said.contains(phrase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_wordings_of_a_missing_repository_are_recognised() {
        for said in [
            "ERROR: Repository not found.",
            "ERROR: The project you were looking for could not be found or you don't have permission to view it.",
            "Gitea: Repository does not exist",
            "fatal: '/srv/x.git' does not appear to be a git repository",
            "Forbidden",
        ] {
            assert!(says_repository_missing(said), "{said}");
        }
        for said in ["", "sh: git-upload-pack: command not found", "fatal: protocol error: bad line length"] {
            assert!(!says_repository_missing(said), "{said}");
        }
    }

    #[test]
    fn reasons_are_saved_by_name() {
        for reason in [RemoteReason::RepositoryNotFound, RemoteReason::EmptyRepository, RemoteReason::Other] {
            assert_eq!(RemoteReason::from_name(reason.name()), Some(reason));
        }
        assert_eq!(RemoteReason::from_name("nonsense"), None);
    }
}

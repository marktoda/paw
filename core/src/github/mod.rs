//! GitHub sign-in: pick the store from a list, and fetch it without the key.
//!
//! paw's GitHub App (`paw-password-store`) is signed in to with GitHub's
//! device flow ([`device`]): paw shows a short code, the person types it at
//! github.com/login/device, and paw receives a user access token and a
//! refresh token. Neither ever reaches Swift; they are kept in a
//! [`session::TokenVault`] the app backs with a keychain item.
//!
//! With the token paw can:
//! - list the repositories the person granted the app ([`repos`]), so setup
//!   offers a list instead of an SSH address to type. Choosing one gives
//!   the SSH address setup already takes: the YubiKey still does the first
//!   clone, over SSH;
//! - fetch the store over HTTPS ([`refresh`]), read-only, through the same
//!   checks and the same integrate as an SSH refresh, so a refresh in the
//!   background needs no key.
//!
//! **The token is read-only and is never used to push.** Pushing stays on
//! SSH with the card's signature. The HTTPS transport refuses
//! `git-receive-pack` outright, and the app asks GitHub for read access to
//! repository contents only.
//!
//! What goes to GitHub, and only to `https://github.com` and
//! `https://api.github.com` (hard-coded; a test build may point them at a
//! simulated GitHub through [`Endpoints`], which a device build without
//! the `sim` feature cannot do):
//! - signing in: the app's public client id, then the device code while
//!   waiting for the person;
//! - refreshing the token: the client id and the refresh token;
//! - listing: the access token, to `/user/installations` and
//!   `/user/installations/{id}/repositories`;
//! - fetching: the access token, to `github.com/<owner>/<repo>.git` only;
//! - signing out, if the person asks for the tokens to be revoked: both
//!   tokens, unauthenticated, to `/credentials/revoke`.
//!
//! Every request says `User-Agent: paw` and nothing else about the device.
//! Nothing else is sent: no telemetry.

pub mod account;
pub mod device;
pub(crate) mod http;
pub mod refresh;
pub mod repos;
pub mod session;
mod transport;

pub use account::{GitHubAccount, SignInExpiry, SignOutReport};
pub use device::{DeviceCode, DeviceSignIn, PollClock, SignInState, SystemPollClock};
pub use refresh::{BackgroundOutcome, BackgroundRefreshReport, BackgroundStop, GitHubLink, RefreshDecision, RefreshStep};
pub use repos::{GitHubRepository, RepositoryList};
pub use session::{TokenVault, VaultError};

use crate::fault::Fault;
use crate::remote::RemoteFailure;

/// paw's GitHub App's client id. Public: it identifies the app, it is not
/// a secret, and paw has no client secret at all.
pub const CLIENT_ID: &str = "Iv23liyBePbnvQPCcGRz";

/// paw's GitHub App's name in URLs.
pub const APP_SLUG: &str = "paw-password-store";

/// Where paw talks to GitHub: the web host (sign-in, token refresh, git).
pub const WEB: &str = "https://github.com";

/// Where paw talks to GitHub: the REST API.
pub const API: &str = "https://api.github.com";

/// The REST API version paw was written against (GitHub's docs, 2026).
pub(crate) const API_VERSION: &str = "2026-03-10";

/// Where paw sends its requests, and as which app. Device builds can only
/// use [`Endpoints::github`]; the `sim` feature adds
/// [`Endpoints::simulated`] for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    web: String,
    api: String,
    client_id: String,
    app_slug: String,
    /// Set only by [`Endpoints::simulated`]. A device build has no field
    /// for it.
    #[cfg(feature = "sim")]
    simulation: Option<Simulation>,
}

/// A test build's simulated GitHub.
#[cfg(feature = "sim")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Simulation {
    /// Its store (`owner/name`) and the SSH address of the simulated host
    /// it is cloned from.
    store: Option<(String, String)>,
}

impl Endpoints {
    /// The real GitHub, as paw's app.
    pub fn github() -> Self {
        Self {
            web: WEB.into(),
            api: API.into(),
            client_id: CLIENT_ID.into(),
            app_slug: APP_SLUG.into(),
            #[cfg(feature = "sim")]
            simulation: None,
        }
    }

    /// A simulated GitHub at `web` and `api` (plain `http://127.0.0.1:...`
    /// is allowed here and nowhere else), as the app `client_id`, whose
    /// repository `ssh_address.0` is cloned over SSH from `ssh_address.1`
    /// (a simulated host). Test builds only.
    #[cfg(feature = "sim")]
    pub fn simulated(web: &str, api: &str, client_id: &str, ssh_address: Option<(String, String)>) -> Self {
        Self {
            web: web.trim_end_matches('/').into(),
            api: api.trim_end_matches('/').into(),
            client_id: client_id.into(),
            app_slug: APP_SLUG.into(),
            simulation: Some(Simulation { store: ssh_address }),
        }
    }

    /// Whether these are a test build's simulated endpoints.
    #[cfg(feature = "sim")]
    pub(crate) fn is_simulated(&self) -> bool {
        self.simulation.is_some()
    }

    /// Whether these are a test build's simulated endpoints: never, in a
    /// device build.
    #[cfg(not(feature = "sim"))]
    pub(crate) fn is_simulated(&self) -> bool {
        false
    }

    pub fn web(&self) -> &str {
        &self.web
    }

    pub fn api(&self) -> &str {
        &self.api
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Where the person installs the app on a repository.
    pub fn install_url(&self) -> String {
        format!("{}/apps/{}/installations/new", self.web, self.app_slug)
    }

    /// Where the person sees and removes the GitHub Apps they authorised.
    pub(crate) fn authorizations_url(&self) -> String {
        format!("{}/settings/apps/authorizations", self.web)
    }

    pub(crate) fn app_slug(&self) -> &str {
        &self.app_slug
    }

    /// The SSH address setup clones `owner/repo` from: GitHub's, except in
    /// a simulation, whose repositories live on a simulated SSH host.
    pub(crate) fn ssh_address(&self, full_name: &str) -> Option<String> {
        #[cfg(feature = "sim")]
        if let Some((name, address)) = self.simulation.as_ref().and_then(|s| s.store.as_ref())
            && name.eq_ignore_ascii_case(full_name)
        {
            return Some(address.clone());
        }
        crate::sync::address::github_repository(full_name).map(|address| address.display())
    }
}

/// Why a GitHub step did not happen. The password store is never changed
/// by one of these.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum GitHubError {
    /// The app's settings on GitHub do not allow device sign-in.
    #[error("GitHub sign-in is turned off for paw's app")]
    DeviceFlowDisabled,
    /// `select_repository` was given a name the last list did not have.
    #[error("{full_name} is not in the list; list the repositories again")]
    NotListed { full_name: String },
    /// The keychain could not keep or read the sign-in.
    #[error("the sign-in could not be kept: {message}")]
    Vault { message: String },
    /// Underneath: GitHub, the network, the sign-in, the chosen
    /// repository, or the store on this device.
    #[error(transparent)]
    Fault { fault: Fault },
}

crate::fault::workflow_error!(GitHubError);

/// The host GitHub's failures name.
const GITHUB: &str = "github.com";

impl GitHubError {
    /// GitHub could not be reached, or answered as if down.
    pub(crate) fn network(detail: impl Into<String>) -> Self {
        RemoteFailure::Unreachable { host: GITHUB.into(), detail: detail.into() }.into()
    }

    /// GitHub answered something paw does not accept: `message` says what,
    /// never with a token.
    pub(crate) fn refused(message: impl Into<String>) -> Self {
        RemoteFailure::GitHubRefused { message: message.into() }.into()
    }
}

/// The sign-in's failure as it reaches a fetch over HTTPS: its fault, or,
/// for the keychain (the only one of GitHub's own cases a fetch can meet),
/// a local one.
impl From<GitHubError> for crate::sync::SyncError {
    fn from(error: GitHubError) -> Self {
        match error {
            GitHubError::Fault { fault } => fault.into(),
            other @ (GitHubError::Vault { .. } | GitHubError::DeviceFlowDisabled | GitHubError::NotListed { .. }) => {
                Fault::local(other).into()
            }
        }
    }
}

/// A token or code, kept in memory that is wiped when dropped. It has no
/// `Display`, and its `Debug` shows nothing of it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Secret(zeroize::Zeroizing<String>);

impl Secret {
    pub(crate) fn new(text: String) -> Self {
        Self(zeroize::Zeroizing::new(text))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// Whether this looks like a token GitHub issues: ASCII letters, digits
    /// and `_-.`, 1 to 1024 long. Anything else is refused before it is
    /// stored or sent in a header.
    pub(crate) fn well_formed(&self) -> bool {
        let text = self.expose();
        !text.is_empty()
            && text.len() <= 1024
            && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_out_of_github_points_at_githubs_own_authorisations_page() {
        assert_eq!(Endpoints::github().authorizations_url(), "https://github.com/settings/apps/authorizations");
        assert!(!Endpoints::github().is_simulated());
    }
}

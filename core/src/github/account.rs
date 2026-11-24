//! The GitHub sign-in of one store, as the app uses it: sign in, list,
//! choose, refresh, sign out. `crate::ffi::github` is a thin wrapper.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

use super::device::{DeviceSignIn, PollClock, SystemPollClock};
use super::http::Http;
use super::refresh::GitHubLink;
use super::repos::{self, GitHubRepository, RepositoryList, is_unauthorised};
use super::session::{TokenKeeper, TokenVault, WallClock, system_wall_clock};
use super::{AUTHORIZATIONS_URL, Endpoints, GitHubError, Secret};

/// What signing out did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SignOutReport {
    /// GitHub accepted the request to revoke both tokens. `false` if it
    /// was not asked, or did not answer.
    pub revoked: bool,
    /// Where the person can remove paw's authorisation for good
    /// (revoking tokens does not remove the app's authorisation, and
    /// paw has no client secret to do it).
    pub manage_url: String,
}

/// The GitHub sign-in kept for the store in `state_dir`.
pub struct GitHubAccount {
    pub(crate) endpoints: Endpoints,
    pub(crate) keeper: Arc<TokenKeeper>,
    pub(crate) state_dir: PathBuf,
    clock: Arc<dyn PollClock>,
    /// The last list shown, which `select_repository` picks from.
    listed: Mutex<Vec<GitHubRepository>>,
    /// Lowers the download cap, for tests.
    pub(crate) download_limit: Mutex<Option<u64>>,
}

impl GitHubAccount {
    /// The real GitHub, with the system clocks.
    pub fn new(state_dir: &Path, vault: Arc<dyn TokenVault>) -> Self {
        Self::with(Endpoints::github(), state_dir, vault, Arc::new(SystemPollClock::default()), system_wall_clock())
    }

    /// Any endpoints and clocks. The endpoints can only be GitHub's
    /// without the `sim` feature.
    pub fn with(
        endpoints: Endpoints,
        state_dir: &Path,
        vault: Arc<dyn TokenVault>,
        clock: Arc<dyn PollClock>,
        wall: WallClock,
    ) -> Self {
        Self {
            endpoints,
            keeper: Arc::new(TokenKeeper::new(vault, wall)),
            state_dir: state_dir.to_path_buf(),
            clock,
            listed: Mutex::new(Vec::new()),
            download_limit: Mutex::new(None),
        }
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// Lowers the most a fetch may receive, in bytes (tests).
    pub fn set_download_limit(&self, bytes: u64) {
        *self.download_limit.lock().unwrap_or_else(|p| p.into_inner()) = Some(bytes);
    }

    /// Whether a sign-in is kept. Reads the vault only.
    pub fn signed_in(&self) -> Result<bool, GitHubError> {
        self.keeper.signed_in()
    }

    /// When the kept sign-in's tokens stop working, in seconds since 1970:
    /// `(access, renewal)`, each `None` if GitHub gave no end. `None` when
    /// signed out. Reads the vault only; the tokens stay in the core.
    pub fn sign_in_expiry(&self) -> Result<Option<(Option<u64>, Option<u64>)>, GitHubError> {
        Ok(self.keeper.load()?.map(|tokens| (tokens.access_expires_at, tokens.refresh_expires_at)))
    }

    /// Starts a device sign-in; see [`DeviceSignIn`].
    pub fn start_sign_in(&self) -> Result<DeviceSignIn, GitHubError> {
        DeviceSignIn::start(&self.endpoints, self.keeper.clone(), self.clock.clone())
    }

    /// Runs `call` with an access token; if GitHub refuses it (401), once
    /// more with a refreshed one.
    pub(crate) fn with_token<T>(&self, http: &Http, call: impl Fn(&Secret) -> Result<T, GitHubError>) -> Result<T, GitHubError> {
        let token = self.keeper.access_token(http, None)?;
        match call(&token) {
            Err(e) if is_unauthorised(&e) => {
                let token = self.keeper.access_token(http, Some(&token))?;
                call(&token)
            }
            other => other,
        }
    }

    /// The repositories the person granted paw, for the picker. Nothing is
    /// sent without a sign-in ([`GitHubError::NotSignedIn`]).
    pub fn repositories(&self) -> Result<RepositoryList, GitHubError> {
        if !self.keeper.signed_in()? {
            return Err(GitHubError::NotSignedIn);
        }
        let http = Http::new(&self.endpoints, None);
        let list = self.with_token(&http, |token| repos::list(&http, token))?;
        let shown = match &list {
            RepositoryList::Repositories { repositories, .. } => repositories.clone(),
            RepositoryList::NotInstalled { .. } => Vec::new(),
        };
        *self.listed.lock().unwrap_or_else(|p| p.into_inner()) = shown;
        Ok(list)
    }

    /// Chooses `full_name` from the last list for this store: remembers it
    /// (the token fetch is for it alone) and returns the SSH address to
    /// give setup (`Setup.set_address`), which continues as if it had been
    /// typed. The key still does the first clone, over SSH.
    pub fn select_repository(&self, full_name: &str) -> Result<String, GitHubError> {
        let listed = self.listed.lock().unwrap_or_else(|p| p.into_inner());
        let chosen = listed
            .iter()
            .find(|r| r.full_name.eq_ignore_ascii_case(full_name))
            .ok_or_else(|| GitHubError::Failed { message: format!("{full_name} is not in the list; list the repositories again") })?;
        let link = GitHubLink { full_name: chosen.full_name.clone(), id: chosen.id, ssh_address: chosen.ssh_address.clone() };
        let _held = crate::lock::StoreLock::for_state_dir(&self.state_dir)
            .exclusive()
            .map_err(|e| GitHubError::Failed { message: e.to_string() })?;
        link.save(&self.state_dir)?;
        Ok(chosen.ssh_address.clone())
    }

    /// The repository chosen for this store, if any.
    pub fn link(&self) -> Option<GitHubLink> {
        GitHubLink::load(&self.state_dir)
    }

    /// Forgets the sign-in here, and, if `revoke`, asks GitHub to revoke
    /// both tokens (`POST /credentials/revoke`, which takes no credentials
    /// of paw's own; GitHub then tells the account's owner by email). The
    /// tokens are forgotten even if that request fails. The chosen
    /// repository is kept.
    pub fn sign_out(&self, revoke: bool) -> Result<SignOutReport, GitHubError> {
        // After any refresh in this process, so none brings the pair back.
        let tokens = self.keeper.take()?;
        let manage_url = if self.endpoints.is_simulated() {
            format!("{}/settings/apps/authorizations", self.endpoints.web())
        } else {
            AUTHORIZATIONS_URL.to_owned()
        };
        let mut revoked = false;
        if let (true, Some(tokens)) = (revoke, tokens) {
            let mut body = Zeroizing::new(String::from("{\"credentials\":[\""));
            // Well-formed tokens need no JSON escaping.
            body.push_str(tokens.access.expose());
            if let Some(refresh) = &tokens.refresh {
                body.push_str("\",\"");
                body.push_str(refresh.expose());
            }
            body.push_str("\"]}");
            let http = Http::new(&self.endpoints, None);
            revoked = http.api_post_anonymous("/credentials/revoke", body.as_bytes()).is_ok_and(|a| a.status == 202);
        }
        Ok(SignOutReport { revoked, manage_url })
    }
}

//! The GitHub sign-in of one store, as the app uses it: sign in, list,
//! choose, refresh, sign out.
//!
//! The tokens never cross into Swift: Swift only provides where they are
//! kept ([`TokenVault`]). The token is read-only and is never used to push.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

use super::device::{DeviceSignIn, PollClock, SystemPollClock};
use super::http::Http;
use super::refresh::GitHubLink;
use super::repos::{self, GitHubRepository, RepositoryList};
use super::session::{TokenKeeper, TokenVault, WallClock, system_wall_clock};
use super::{Endpoints, GitHubError, Secret};
use crate::remote::{AboutRemote, RemoteFailure};

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

/// When a kept GitHub sign-in stops working, seconds since 1970.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SignInExpiry {
    /// The access token; GitHub renews it with the renewal token.
    pub access_until_secs: Option<u64>,
    /// The renewal token: after this, sign in again.
    pub renewal_until_secs: Option<u64>,
}

/// The GitHub sign-in kept for the store in `state_dir`.
#[derive(uniffi::Object)]
pub struct GitHubAccount {
    pub(crate) endpoints: Endpoints,
    pub(crate) keeper: Arc<TokenKeeper>,
    pub(crate) state_dir: PathBuf,
    clock: Arc<dyn PollClock>,
    /// The last list shown, which `select_repository` picks from.
    listed: Mutex<Vec<GitHubRepository>>,
    #[cfg(feature = "sim")]
    download_limit: Mutex<Option<u64>>,
}

impl GitHubAccount {
    pub fn new(state_dir: &Path, vault: Arc<dyn TokenVault>) -> Self {
        Self::with(Endpoints::github(), state_dir, vault, Arc::new(SystemPollClock::default()), system_wall_clock())
    }

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
            #[cfg(feature = "sim")]
            download_limit: Mutex::new(None),
        }
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    #[cfg(feature = "sim")]
    pub fn set_download_limit(&self, bytes: u64) {
        *self.download_limit.lock().unwrap_or_else(|p| p.into_inner()) = Some(bytes);
    }

    pub(crate) fn download_limit(&self) -> Option<u64> {
        #[cfg(feature = "sim")]
        return *self.download_limit.lock().unwrap_or_else(|p| p.into_inner());
        #[cfg(not(feature = "sim"))]
        None
    }

    /// Runs `call` with an access token; if GitHub refuses it
    /// ([`RemoteFailure::Unauthorised`]), once more with a refreshed one.
    pub(crate) fn with_token<T, E: From<GitHubError> + AboutRemote>(
        &self,
        http: &Http,
        mut call: impl FnMut(Secret) -> Result<T, E>,
    ) -> Result<T, E> {
        let token = self.keeper.access_token(http, None)?;
        match call(token.clone()) {
            Err(e) if e.is_unauthorised() => {
                let token = self.keeper.access_token(http, Some(&token))?;
                call(token)
            }
            other => other,
        }
    }


}

#[uniffi::export]
impl GitHubAccount {
    #[uniffi::constructor]
    pub fn open(state_dir: String, vault: Arc<dyn TokenVault>) -> Arc<Self> {
        Arc::new(Self::new(Path::new(&state_dir), vault))
    }

    /// Whether a sign-in is kept. Reads the vault only; sends nothing.
    pub fn is_signed_in(&self) -> Result<bool, GitHubError> {
        self.keeper.signed_in()
    }

    /// When the kept sign-in ends, for Copy diagnostics: `None` when
    /// signed out. Reads the vault only; sends nothing, shows no token.
    pub fn sign_in_expiry(&self) -> Result<Option<SignInExpiry>, GitHubError> {
        Ok(self.keeper.load()?.map(|tokens| SignInExpiry {
            access_until_secs: tokens.access_expires_at,
            renewal_until_secs: tokens.refresh_expires_at,
        }))
    }

    /// Starts a device sign-in: one request to GitHub with paw's client
    /// id; see [`DeviceSignIn`].
    pub fn start_sign_in(&self) -> Result<Arc<DeviceSignIn>, GitHubError> {
        DeviceSignIn::start(&self.endpoints, self.keeper.clone(), self.clock.clone()).map(Arc::new)
    }

    /// The repositories the person granted paw, for the picker. Nothing is
    /// sent without a sign-in ([`RemoteFailure::NotSignedIn`]).
    pub fn repositories(&self) -> Result<RepositoryList, GitHubError> {
        if !self.keeper.signed_in()? {
            return Err(RemoteFailure::NotSignedIn.into());
        }
        let http = Http::new(&self.endpoints, None);
        let list = self.with_token(&http, |token| repos::list(&http, &token))?;
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
    /// typed. The key still does the first clone, over SSH. The token is
    /// read-only and is never used to push.
    pub fn select_repository(&self, full_name: &str) -> Result<String, GitHubError> {
        let listed = self.listed.lock().unwrap_or_else(|p| p.into_inner());
        let chosen = listed
            .iter()
            .find(|r| r.full_name.eq_ignore_ascii_case(full_name))
            .ok_or_else(|| GitHubError::NotListed { full_name: full_name.to_owned() })?;
        let link = GitHubLink { full_name: chosen.full_name.clone(), id: chosen.id, ssh_address: chosen.ssh_address.clone() };
        let _held = crate::lock::StoreLock::for_state_dir(&self.state_dir).exclusive()?;
        link.save(&self.state_dir)?;
        Ok(chosen.ssh_address.clone())
    }

    pub fn linked_repository(&self) -> Option<GitHubLink> {
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
        let manage_url = self.endpoints.authorizations_url();
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

    pub fn install_url(&self) -> String {
        self.endpoints.install_url()
    }
}

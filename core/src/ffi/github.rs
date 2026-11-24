//! GitHub sign-in as Swift sees it; see [`crate::github`].
//!
//! The tokens never cross into Swift: Swift only provides where they are
//! kept ([`TokenVault`]). The token is read-only and is never used to push.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::github::{
    BackgroundRefreshReport, DeviceCode, DeviceSignIn, GitHubAccount, GitHubError, GitHubLink, RepositoryList,
    SignInState, SignOutReport, TokenVault,
};
use crate::sync::SyncError;
use crate::sync::operation::SyncReport;

/// When a kept GitHub sign-in stops working, seconds since 1970.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SignInExpiry {
    /// The access token; GitHub renews it with the renewal token.
    pub access_until_secs: Option<u64>,
    /// The renewal token: after this, sign in again.
    pub renewal_until_secs: Option<u64>,
}

/// The GitHub sign-in of the store kept in `state_dir`.
#[derive(uniffi::Object)]
pub struct GitHub {
    account: GitHubAccount,
}

/// One device sign-in. Call `poll` in a loop on a background thread while
/// it returns `Waiting`; `cancel` from any thread.
#[derive(uniffi::Object)]
pub struct GitHubSignIn {
    inner: DeviceSignIn,
}

#[uniffi::export]
impl GitHubSignIn {
    /// The code to show and where to type it.
    pub fn code(&self) -> DeviceCode {
        self.inner.code()
    }

    /// Waits until the next poll is due (GitHub's interval, longer after a
    /// `slow_down`), asks GitHub once, and says where sign-in stands. On
    /// `SignedIn` the tokens are already in the vault.
    pub fn poll(&self) -> Result<SignInState, GitHubError> {
        self.inner.poll()
    }

    /// Ends the sign-in; a `poll` waiting now returns `Cancelled` at once.
    pub fn cancel(&self) {
        self.inner.cancel();
    }
}

#[uniffi::export]
impl GitHub {
    /// The sign-in for the store in `state_dir`, kept in `vault`, against
    /// the real GitHub.
    #[uniffi::constructor]
    pub fn open(state_dir: String, vault: Arc<dyn TokenVault>) -> Arc<Self> {
        Arc::new(Self { account: GitHubAccount::new(&PathBuf::from(state_dir), vault) })
    }

    /// Whether a sign-in is kept. Reads the vault only; sends nothing.
    pub fn is_signed_in(&self) -> Result<bool, GitHubError> {
        self.account.signed_in()
    }

    /// When the kept sign-in ends, for Copy diagnostics: `None` when
    /// signed out. Reads the vault only; sends nothing, shows no token.
    pub fn sign_in_expiry(&self) -> Result<Option<SignInExpiry>, GitHubError> {
        Ok(self.account.sign_in_expiry()?.map(|(access, renewal)| SignInExpiry {
            access_until_secs: access,
            renewal_until_secs: renewal,
        }))
    }

    /// Starts a device sign-in: one request to GitHub with paw's client id.
    pub fn start_sign_in(&self) -> Result<Arc<GitHubSignIn>, GitHubError> {
        Ok(Arc::new(GitHubSignIn { inner: self.account.start_sign_in()? }))
    }

    /// The repositories the person granted paw, for the picker; or
    /// `NotInstalled` with the page to install paw on a repository.
    pub fn repositories(&self) -> Result<RepositoryList, GitHubError> {
        self.account.repositories()
    }

    /// Chooses `full_name` (`owner/name`) from the last `repositories`
    /// list and returns its SSH address: pass it to `Setup.set_address`
    /// and setup continues as if it had been typed. The token is
    /// read-only and is never used to push.
    pub fn select_repository(&self, full_name: String) -> Result<String, GitHubError> {
        self.account.select_repository(&full_name)
    }

    /// The repository chosen for this store, if any.
    pub fn linked_repository(&self) -> Option<GitHubLink> {
        self.account.link()
    }

    /// Fetches over HTTPS with the sign-in (no key) and integrates exactly
    /// as `StoreSync.refresh` does; pushes nothing. Sign-in problems come
    /// as `Failed` (or `Network`); `is_signed_in` then says whether the
    /// person must sign in again.
    pub fn refresh_store(&self) -> Result<SyncReport, SyncError> {
        self.account.refresh_with_token()
    }

    /// The background refresh: at most `seconds` long, never asks anyone
    /// anything, never pushes, and leaves every decision to the person
    /// (see `BackgroundRefreshReport.decisions`). Safe to be killed.
    pub fn background_refresh(&self, seconds: u32) -> BackgroundRefreshReport {
        self.account.background_refresh(Duration::from_secs(u64::from(seconds)))
    }

    /// Forgets the sign-in on this device. With `revoke`, also asks GitHub
    /// to revoke both tokens (GitHub emails the account's owner that it
    /// did). Open `manage_url` to remove paw's authorisation for good.
    pub fn sign_out(&self, revoke: bool) -> Result<SignOutReport, GitHubError> {
        self.account.sign_out(revoke)
    }

    /// Where the person installs paw's GitHub App on a repository.
    pub fn install_url(&self) -> String {
        self.account.endpoints().install_url()
    }
}

#[cfg(feature = "sim")]
#[uniffi::export]
impl GitHub {
    /// [`GitHub::open`] against a simulated GitHub. Not in device builds.
    #[uniffi::constructor]
    pub fn open_simulated(
        state_dir: String,
        vault: Arc<dyn TokenVault>,
        github: Arc<crate::sim::github::SimGitHub>,
    ) -> Arc<Self> {
        let account = GitHubAccount::with(
            github.endpoints(),
            &PathBuf::from(state_dir),
            vault,
            Arc::new(crate::github::SystemPollClock::default()),
            crate::github::session::system_wall_clock(),
        );
        Arc::new(Self { account })
    }
}

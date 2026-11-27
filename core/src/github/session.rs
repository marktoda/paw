//! Keeping the GitHub sign-in: the tokens, sealed by the phone, and their
//! refresh.
//!
//! The access token lasts 8 hours and the refresh token 6 months. Each
//! refresh returns a new pair and GitHub invalidates the old pair at once
//! ("Once you use a refresh token, that refresh token and the old user
//! access token will no longer work").
//!
//! **One item, replaced whole.** Both tokens and their expiry times are one
//! vault item ([`SESSION_ITEM`]), so a refresh replaces the old pair with
//! the new one in one keychain write: there is never a moment with the new
//! refresh token and the old access token, or with neither. The new access
//! token is used only after that write succeeded. If paw is killed (or the
//! write fails) after GitHub answered and before the write, the old pair is
//! still stored but GitHub no longer accepts it: the next refresh is
//! refused, the sign-in ends cleanly ([`RemoteFailure::SignedOut`]) and the
//! person signs in again. No order of writes avoids that window, because
//! GitHub has already rotated; one write keeps it to one keychain call.
//!
//! Refreshes are serialised in the process (one lock), and each re-reads
//! the vault after taking it, so two callers in one process never spend one
//! refresh token twice.
//!
//! **Between processes** (the app and its extension share the vault) no
//! lock is held: a file lock held across the refresh request would get a
//! process suspended mid-request killed by iOS. If both send the same
//! refresh token, GitHub honours one and refuses the other with
//! `bad_refresh_token`. The refused one then re-reads the vault for up to
//! [`OTHER_PROCESS_GRACE`]: once the winner has kept its new pair, that
//! pair is used. Only if the vault still holds the spent token afterwards
//! is the sign-in over. So the token is still spent once, and the race
//! costs one refused request, not the sign-in.

use crate::remote::RemoteFailure;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use zeroize::Zeroizing;

use super::http::Http;
use super::{GitHubError, Secret};

pub const SESSION_ITEM: &str = "github-session";

const REFRESH_MARGIN_SECS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum VaultError {
    /// `message` must not contain the secret.
    #[error("{message}")]
    Failed { message: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for VaultError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Failed { message: e.reason }
    }
}

/// Where the GitHub sign-in is kept between launches. Implemented in Swift.
///
/// Unlike [`crate::copies::seams::DeviceKeystore`], this must work with
/// nobody present: a background refresh runs while the phone is locked in
/// a pocket. On iOS each item is a keychain generic-password item
/// (`kSecClassGenericPassword`, service `paw.github`, account `name`),
/// `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`, with
/// `kSecAttrSynchronizable` false and no access control (no Face ID, no
/// passcode), in the app-only access group (the AutoFill extension never
/// uses GitHub). `store` replaces an existing item in one `SecItemUpdate`
/// (or `SecItemAdd` if there is none), so a reader sees the old value or
/// the new one, never neither.
#[uniffi::export(with_foreign)]
pub trait TokenVault: Send + Sync {
    fn store(&self, name: String, secret: Vec<u8>) -> Result<(), VaultError>;
    fn load(&self, name: String) -> Result<Option<Vec<u8>>, VaultError>;
    /// Forgets `name`. Not an error if there is nothing.
    fn remove(&self, name: String) -> Result<(), VaultError>;
}

pub(crate) fn vault_err(e: VaultError) -> GitHubError {
    GitHubError::Vault { message: e.to_string() }
}

/// The sign-in as kept: both tokens and when they end (seconds since 1970;
/// `None` if GitHub said they do not expire).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tokens {
    pub access: Secret,
    pub access_expires_at: Option<u64>,
    pub refresh: Option<Secret>,
    pub refresh_expires_at: Option<u64>,
}

const HEADER: &str = "paw-github-session 1";

impl Tokens {
    fn encode(&self) -> Zeroizing<Vec<u8>> {
        let mut text = Zeroizing::new(String::new());
        text.push_str(HEADER);
        text.push('\n');
        text.push_str("access=");
        text.push_str(self.access.expose());
        text.push('\n');
        if let Some(at) = self.access_expires_at {
            text.push_str(&format!("access_expires={at}\n"));
        }
        if let Some(refresh) = &self.refresh {
            text.push_str("refresh=");
            text.push_str(refresh.expose());
            text.push('\n');
        }
        if let Some(at) = self.refresh_expires_at {
            text.push_str(&format!("refresh_expires={at}\n"));
        }
        Zeroizing::new(text.as_bytes().to_vec())
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut lines = text.lines();
        if lines.next()? != HEADER {
            return None;
        }
        let mut tokens = Tokens { access: Secret::new(String::new()), access_expires_at: None, refresh: None, refresh_expires_at: None };
        for line in lines {
            let (name, value) = line.split_once('=')?;
            match name {
                "access" => tokens.access = Secret::new(value.to_owned()),
                "access_expires" => tokens.access_expires_at = Some(value.parse().ok()?),
                "refresh" => tokens.refresh = Some(Secret::new(value.to_owned())),
                "refresh_expires" => tokens.refresh_expires_at = Some(value.parse().ok()?),
                _ => {}
            }
        }
        tokens.usable()
    }

    /// `self` if both tokens are well formed: nothing else is kept or sent.
    fn usable(self) -> Option<Self> {
        let refresh_ok = self.refresh.as_ref().is_none_or(Secret::well_formed);
        (self.access.well_formed() && refresh_ok).then_some(self)
    }

    pub(crate) fn from_answer(mut answer: TokenAnswer, now: u64) -> Option<Self> {
        let access = Secret::new(answer.access_token.take()?);
        let refresh = answer.refresh_token.take().map(Secret::new);
        let tokens = Tokens {
            access,
            access_expires_at: answer.expires_in.map(|s| now.saturating_add(s)),
            refresh,
            refresh_expires_at: answer.refresh_token_expires_in.map(|s| now.saturating_add(s)),
        };
        tokens.usable()
    }
}

/// GitHub's answer at `/login/oauth/access_token`: tokens, or an error.
#[derive(Deserialize, Default)]
pub(crate) struct TokenAnswer {
    pub access_token: Option<String>,
    pub expires_in: Option<u64>,
    pub refresh_token: Option<String>,
    pub refresh_token_expires_in: Option<u64>,
    pub error: Option<String>,
    /// With `slow_down`: the interval to use from now on.
    pub interval: Option<u64>,
}

impl Drop for TokenAnswer {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(token) = &mut self.access_token {
            token.zeroize();
        }
        if let Some(token) = &mut self.refresh_token {
            token.zeroize();
        }
    }
}

impl std::fmt::Debug for TokenAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenAnswer")
            .field("has_access_token", &self.access_token.is_some())
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("error", &self.error)
            .finish()
    }
}

pub(crate) fn parse_token_answer(body: &[u8]) -> Result<TokenAnswer, GitHubError> {
    serde_json::from_slice(body).map_err(|_| GitHubError::refused("GitHub's sign-in answer was not readable"))
}

/// Seconds since 1970 now.
pub type WallClock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub fn system_wall_clock() -> WallClock {
    Arc::new(|| SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
}

static REFRESHING: Mutex<()> = Mutex::new(());

/// How long a refresh GitHub refused as spent waits for another process
/// that spent the same token to keep its new pair. That process is past
/// its request (GitHub answered it first) and keeps the pair at once.
pub const OTHER_PROCESS_GRACE: Duration = Duration::from_secs(1);

/// The sign-in kept in a vault, and the one way to get a usable access
/// token from it.
pub(crate) struct TokenKeeper {
    vault: Arc<dyn TokenVault>,
    wall: WallClock,
}

impl TokenKeeper {
    pub(crate) fn new(vault: Arc<dyn TokenVault>, wall: WallClock) -> Self {
        Self { vault, wall }
    }

    /// Forgets the sign-in, after any refresh in this process has kept its
    /// pair (so a refresh finishing later cannot bring it back).
    pub(crate) fn take(&self) -> Result<Option<Tokens>, GitHubError> {
        let _one_at_a_time = REFRESHING.lock().unwrap_or_else(|p| p.into_inner());
        let tokens = self.load()?;
        self.forget()?;
        Ok(tokens)
    }

    /// After GitHub refused `spent` as already used: the pair another
    /// process kept meanwhile, if one appears within
    /// [`OTHER_PROCESS_GRACE`]; else the sign-in is over and forgotten.
    fn after_spent(&self, spent: &Secret) -> Result<Secret, GitHubError> {
        let deadline = std::time::Instant::now() + OTHER_PROCESS_GRACE;
        loop {
            match self.load()? {
                None => return Err(GitHubError::from(RemoteFailure::SignedOut)),
                Some(kept) if kept.refresh.as_ref() != Some(spent) => return Ok(kept.access),
                Some(_) if std::time::Instant::now() >= deadline => break,
                Some(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        self.forget()?;
        Err(GitHubError::from(RemoteFailure::SignedOut))
    }

    pub(crate) fn now(&self) -> u64 {
        (self.wall)()
    }

    /// The kept sign-in, if any. A damaged item counts as none.
    pub(crate) fn load(&self) -> Result<Option<Tokens>, GitHubError> {
        let stored = self.vault.load(SESSION_ITEM.into()).map_err(vault_err)?;
        Ok(stored.map(Zeroizing::new).and_then(|bytes| Tokens::decode(&bytes)))
    }

    pub(crate) fn signed_in(&self) -> Result<bool, GitHubError> {
        Ok(self.load()?.is_some())
    }

    pub(crate) fn save(&self, tokens: &Tokens) -> Result<(), GitHubError> {
        let bytes = tokens.encode();
        self.vault.store(SESSION_ITEM.into(), bytes.to_vec()).map_err(vault_err)
    }

    pub(crate) fn forget(&self) -> Result<(), GitHubError> {
        self.vault.remove(SESSION_ITEM.into()).map_err(vault_err)
    }

    /// A usable access token: the kept one, or, if it ends within five
    /// minutes (or it is `refused`, the one GitHub just refused), a
    /// refreshed one, stored before it is returned. If the kept one is no
    /// longer the refused one, another caller (thread or process) has
    /// refreshed meanwhile, and its new token is used: refreshing again
    /// would spend the refresh token a second time and end that caller's
    /// fetch.
    pub(crate) fn access_token(&self, http: &Http, refused: Option<&Secret>) -> Result<Secret, GitHubError> {
        let _one_at_a_time = REFRESHING.lock().unwrap_or_else(|p| p.into_inner());
        let Some(tokens) = self.load()? else {
            return Err(GitHubError::from(RemoteFailure::NotSignedIn));
        };
        let force = refused.is_some_and(|refused| *refused == tokens.access);
        let now = self.now();
        let fresh = tokens.access_expires_at.is_none_or(|at| at > now.saturating_add(REFRESH_MARGIN_SECS));
        if fresh && !force {
            return Ok(tokens.access);
        }
        let Some(refresh) = tokens.refresh.clone() else {
            if force || !fresh {
                self.forget()?;
                return Err(GitHubError::from(RemoteFailure::SignedOut));
            }
            return Ok(tokens.access);
        };
        if tokens.refresh_expires_at.is_some_and(|at| at <= now) {
            self.forget()?;
            return Err(GitHubError::from(RemoteFailure::SignedOut));
        }
        let client_id = http.endpoints().client_id().to_owned();
        let answer = http.post_form(
            "/login/oauth/access_token",
            &[
                ("client_id", &client_id),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.expose()),
            ],
        )?;
        GitHubError::unless_down(answer.status)?;
        let parsed = parse_token_answer(&answer.body)?;
        match parsed.error.as_deref() {
            None if answer.status < 400 => {}
            // Spent (perhaps by the other process, just now), expired or
            // revoked (the person removed paw at GitHub).
            Some("bad_refresh_token") => return self.after_spent(&refresh),
            // Anything else is not the sign-in's fault (a setting of the
            // app, a rate limit): keep it, and say so.
            other => {
                let code: String = other.unwrap_or("none").chars().filter(|c| c.is_ascii_lowercase() || *c == '_').take(64).collect();
                return Err(GitHubError::refused(format!("GitHub would not refresh the sign-in ({code}, {})", answer.status)));
            }
        }
        drop(refresh);
        let Some(new) = Tokens::from_answer(parsed, self.now()) else {
            return Err(GitHubError::refused("GitHub's refreshed sign-in was not usable"));
        };
        self.save(&new)?;
        Ok(new.access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_survives_encoding_and_hides_its_tokens() {
        let tokens = Tokens {
            access: Secret::new("ghu_access".into()),
            access_expires_at: Some(100),
            refresh: Some(Secret::new("ghr_refresh".into())),
            refresh_expires_at: Some(200),
        };
        let bytes = tokens.encode();
        assert_eq!(Tokens::decode(&bytes), Some(tokens.clone()));
        let shown = format!("{tokens:?}");
        assert!(!shown.contains("ghu_access") && !shown.contains("ghr_refresh"), "{shown}");
        assert_eq!(Tokens::decode(b"something else"), None);
        assert_eq!(Tokens::decode(b"paw-github-session 1\naccess=bad token\n"), None);
    }

    #[test]
    fn a_token_answer_hides_its_tokens() {
        let answer = parse_token_answer(br#"{"access_token":"ghu_x","refresh_token":"ghr_y","expires_in":28800}"#).unwrap();
        let shown = format!("{answer:?}");
        assert!(!shown.contains("ghu_x") && !shown.contains("ghr_y"), "{shown}");
    }
}

//! Signing in to paw's GitHub App with the device flow, as a state machine
//! the UI drives.
//!
//! ```text
//! start ─► (show code) ─► poll ─► Waiting ─► poll ─► ... ─┬─► SignedIn
//!                                                         ├─► Denied
//!                                                         ├─► Expired
//!                                                         └─► Cancelled (cancel, from any thread)
//! ```
//!
//! [`DeviceSignIn::start`] asks GitHub for a code (`POST /login/device/code`
//! with the client id and nothing else). The person types the code at the
//! address shown. [`DeviceSignIn::poll`] waits until the next poll is
//! allowed (the interval GitHub gave, plus 5 seconds after each
//! `slow_down`), asks once (`POST /login/oauth/access_token` with the
//! client id and the device code), and says where sign-in stands. Swift
//! calls `poll` in a loop on a background thread until the state is no
//! longer `Waiting`; `cancel` from any thread ends the wait at once. The
//! tokens go straight into the vault; they are never returned.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::http::Http;
use super::session::{TokenKeeper, Tokens, parse_token_answer};
use super::{Endpoints, GitHubError, Secret};

/// What GitHub adds to the interval when it says `slow_down`.
pub const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);
/// The interval if GitHub names none (its documented default).
const DEFAULT_INTERVAL: u64 = 5;
/// Never poll faster than this, whatever an answer says.
const MIN_INTERVAL: u64 = 1;

/// What to show the person.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceCode {
    /// The code to type, e.g. `WDJB-MJHT`.
    pub user_code: String,
    /// Where to type it: `https://github.com/login/device`.
    pub verification_uri: String,
    /// How long the code lasts, in seconds from `start`.
    pub expires_in: u32,
    /// The seconds between polls GitHub asked for at the start.
    pub interval: u32,
}

/// Where a sign-in stands after a poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SignInState {
    /// The person has not finished at GitHub yet: poll again.
    Waiting,
    /// Signed in; the tokens are in the vault.
    SignedIn,
    /// The person pressed Cancel at GitHub. The code cannot be used again.
    Denied,
    /// The code ran out before the person finished. Start again.
    Expired,
    /// `cancel` was called. Nothing was kept.
    Cancelled,
}

/// The time a sign-in waits by. Monotonic; tests use a simulated one so no
/// test sleeps.
pub trait PollClock: Send + Sync {
    /// Time since some fixed start.
    fn now(&self) -> Duration;
    /// Blocks until `now() >= at` or `cancel` is set; `false` if cancelled.
    fn wait_until(&self, at: Duration, cancel: &Cancel) -> bool;
}

/// Set once, from any thread, to end a wait.
#[derive(Default)]
pub struct Cancel {
    set: Mutex<bool>,
    changed: Condvar,
}

impl Cancel {
    pub fn cancel(&self) {
        *self.set.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.changed.notify_all();
    }

    pub fn is_set(&self) -> bool {
        *self.set.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The real clock: `Instant`, waiting on a condition variable so `cancel`
/// wakes it.
pub struct SystemPollClock {
    start: Instant,
}

impl Default for SystemPollClock {
    fn default() -> Self {
        Self { start: Instant::now() }
    }
}

impl PollClock for SystemPollClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }

    fn wait_until(&self, at: Duration, cancel: &Cancel) -> bool {
        let mut set = cancel.set.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if *set {
                return false;
            }
            let now = self.now();
            if now >= at {
                return true;
            }
            set = cancel.changed.wait_timeout(set, at - now).unwrap_or_else(|p| p.into_inner()).0;
        }
    }
}

#[derive(Deserialize)]
struct CodeAnswer {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
    error: Option<String>,
}

impl Drop for CodeAnswer {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(code) = &mut self.device_code {
            code.zeroize();
        }
    }
}

struct Progress {
    interval: Duration,
    next_poll: Duration,
    finished: Option<SignInState>,
    polls: u32,
}

/// One device sign-in. See the module comment.
pub struct DeviceSignIn {
    http: Http,
    keeper: Arc<TokenKeeper>,
    clock: Arc<dyn PollClock>,
    cancel: Cancel,
    device_code: Secret,
    code: DeviceCode,
    expires_at: Duration,
    progress: Mutex<Progress>,
}

impl std::fmt::Debug for DeviceSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceSignIn").field("code", &self.code).finish_non_exhaustive()
    }
}

fn refused(message: &str) -> GitHubError {
    GitHubError::Refused { message: message.into() }
}

/// Codes GitHub shows are short letters, digits and a hyphen; anything else
/// is not shown to the person.
fn plausible_user_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= 32 && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl DeviceSignIn {
    /// Asks GitHub for a code to show. Sends the client id only.
    pub(crate) fn start(endpoints: &Endpoints, keeper: Arc<TokenKeeper>, clock: Arc<dyn PollClock>) -> Result<Self, GitHubError> {
        let http = Http::new(endpoints, None);
        let client_id = endpoints.client_id().to_owned();
        let answer = http.post_form("/login/device/code", &[("client_id", &client_id)])?;
        let started = clock.now();
        if answer.status >= 500 {
            return Err(GitHubError::Network { message: format!("GitHub answered {}", answer.status) });
        }
        let mut parsed: CodeAnswer =
            serde_json::from_slice(&answer.body).map_err(|_| refused("GitHub's sign-in answer was not readable"))?;
        match parsed.error.as_deref() {
            None => {}
            Some("device_flow_disabled") => return Err(GitHubError::DeviceFlowDisabled),
            Some(_) => return Err(refused("GitHub did not start the sign-in")),
        }
        let device_code = Secret::new(parsed.device_code.take().ok_or_else(|| refused("GitHub sent no device code"))?);
        let user_code = parsed.user_code.take().filter(|c| plausible_user_code(c)).ok_or_else(|| refused("GitHub sent no code to show"))?;
        // Shown to the person: it must be GitHub's own page.
        let verification_uri = parsed
            .verification_uri
            .take()
            .filter(|uri| http.may_carry_credentials(uri, super::http::Host::Web))
            .unwrap_or_else(|| format!("{}/login/device", endpoints.web()));
        if !device_code.well_formed() {
            return Err(refused("GitHub's device code was not usable"));
        }
        let expires_in = parsed.expires_in.unwrap_or(900).clamp(1, 3600);
        let interval = parsed.interval.unwrap_or(DEFAULT_INTERVAL).clamp(MIN_INTERVAL, 600);
        let interval_duration = Duration::from_secs(interval);
        Ok(Self {
            http,
            keeper,
            clock,
            cancel: Cancel::default(),
            device_code,
            code: DeviceCode {
                user_code,
                verification_uri,
                expires_in: expires_in as u32,
                interval: interval as u32,
            },
            expires_at: started + Duration::from_secs(expires_in),
            progress: Mutex::new(Progress {
                interval: interval_duration,
                next_poll: started + interval_duration,
                finished: None,
                polls: 0,
            }),
        })
    }

    pub fn code(&self) -> DeviceCode {
        self.code.clone()
    }

    /// The seconds paw waits between polls now (after any `slow_down`).
    pub fn interval(&self) -> Duration {
        self.progress.lock().unwrap_or_else(|p| p.into_inner()).interval
    }

    /// How many times GitHub has been asked for the token.
    pub fn polls(&self) -> u32 {
        self.progress.lock().unwrap_or_else(|p| p.into_inner()).polls
    }

    /// Ends the sign-in: a `poll` waiting now, or any later, returns
    /// `Cancelled`, and tokens that arrive after this are not kept.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Waits until the next poll is due, polls once, and says where the
    /// sign-in stands. Once it is not `Waiting`, it stays there. A network
    /// failure is an error and the next `poll` tries again, an interval
    /// later.
    pub fn poll(&self) -> Result<SignInState, GitHubError> {
        let mut progress = self.progress.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(finished) = progress.finished {
            return Ok(finished);
        }
        let due = progress.next_poll.min(self.expires_at);
        if !self.clock.wait_until(due, &self.cancel) {
            progress.finished = Some(SignInState::Cancelled);
            return Ok(SignInState::Cancelled);
        }
        if self.clock.now() >= self.expires_at {
            progress.finished = Some(SignInState::Expired);
            return Ok(SignInState::Expired);
        }
        progress.polls += 1;
        progress.next_poll = self.clock.now() + progress.interval;
        let client_id = self.http.endpoints().client_id().to_owned();
        let answer = self.http.post_form(
            "/login/oauth/access_token",
            &[
                ("client_id", &client_id),
                ("device_code", self.device_code.expose()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ],
        )?;
        if answer.status >= 500 {
            return Err(GitHubError::Network { message: format!("GitHub answered {}", answer.status) });
        }
        let parsed = parse_token_answer(&answer.body)?;
        let state = match parsed.error.as_deref() {
            Some("authorization_pending") => SignInState::Waiting,
            Some("slow_down") => {
                let slower = progress.interval + SLOW_DOWN_STEP;
                let named = Duration::from_secs(parsed.interval.unwrap_or(0).min(600));
                progress.interval = slower.max(named);
                progress.next_poll = self.clock.now() + progress.interval;
                SignInState::Waiting
            }
            Some("expired_token" | "token_expired") => SignInState::Expired,
            Some("access_denied") => SignInState::Denied,
            Some("device_flow_disabled") => {
                progress.finished = Some(SignInState::Denied);
                return Err(GitHubError::DeviceFlowDisabled);
            }
            Some(_) => {
                // `incorrect_client_credentials`, `incorrect_device_code`,
                // `unsupported_grant_type`: paw's own mistake; never retried.
                progress.finished = Some(SignInState::Expired);
                return Err(refused("GitHub refused the sign-in request"));
            }
            None => {
                let now = self.keeper.now();
                let tokens = Tokens::from_answer(parsed, now).ok_or_else(|| refused("GitHub's sign-in was not usable"))?;
                if self.cancel.is_set() {
                    progress.finished = Some(SignInState::Cancelled);
                    return Ok(SignInState::Cancelled);
                }
                self.keeper.save(&tokens)?;
                SignInState::SignedIn
            }
        };
        if state != SignInState::Waiting {
            progress.finished = Some(state);
        }
        Ok(state)
    }
}

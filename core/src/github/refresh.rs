//! Fetching the store over HTTPS with the GitHub sign-in, and the
//! background refresh built on it.
//!
//! [`GitHubAccount::refresh_store`] is a refresh exactly like the SSH
//! one (`StoreSync.refresh`) with a different transport: the same fetch
//! (every branch into `origin/*`), then the same integrate, so the same
//! trees are refused, the same `.gpg-id` changes reported and a replaced
//! history stops it the same way. It refuses to run unless the store's
//! remote is the repository chosen in the picker.
//!
//! [`GitHubAccount::background_refresh_stoppable`] is what a background task calls,
//! with nobody present. It does what a refresh does without a decision
//! (fast-forward, replay local changes), and parks the rest for the
//! person, changing nothing:
//! - the fetched tree holds something paw will not check out;
//! - a `.gpg-id` changed (it is not integrated at all in the background,
//!   so the foreground refresh shows it as usual);
//! - the remote's history was replaced;
//! - waiting changes are held back (reported; it never pushes anyway).
//!
//! It is bounded by a deadline and stops between whole steps once past it
//! or once its `stop` says so (the transfer is also cut at the next chunk);
//! every request is cut at the deadline too ([`Http`]). It waits at most
//! [`BACKGROUND_LOCK_WAIT`] for the store or another fetch, and ends `Busy`
//! instead. The store lock is held for the local steps only (publish,
//! check, integrate), never across the network.
//! Being killed at any point leaves the store consistent: the token
//! rotation is one vault write; libgit2 writes the pack and then the
//! `origin/*` refs; integrate moves branch, index and working copy last,
//! together (`sync::integrate`); a fetch without its integrate is picked
//! up by the next refresh.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::account::GitHubAccount;
use super::http::Http;
use super::transport::{Failure, HttpsRemote, explain};
use super::{GitHubError, Secret};
use crate::PawError;
use crate::lock::{Exclusive, Fetching, StoreLock};
use crate::sync::address::{StoreAddress, github_repository, saved_address};
use crate::sync::integrate::{IntegrateReport, RecipientsChange, Replaced};
use crate::sync::operation::{SyncReport, SyncState};
use crate::sync::{RefusedPath, SyncError, TransferProgress};

/// The longest a background refresh waits for the store lock or for
/// another fetch of the store: past it the run ends `Busy`, having changed
/// nothing. Short, because the app in front (a save, a sync) comes first.
pub const BACKGROUND_LOCK_WAIT: Duration = Duration::from_secs(3);

/// The GitHub repository chosen for a store. Not secret.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GitHubLink {
    /// `owner/name`.
    pub full_name: String,
    pub id: u64,
    /// The SSH address setup was given for it.
    pub ssh_address: String,
}

impl GitHubLink {
    pub(crate) fn save(&self, state_dir: &Path) -> Result<(), GitHubError> {
        let failed = |e: std::io::Error| GitHubError::Failed { message: format!("could not save the chosen repository: {e}") };
        std::fs::create_dir_all(state_dir).map_err(failed)?;
        let text = format!(
            "repository={}\nid={}\nssh={}\n",
            self.full_name.replace(['\n', '\r'], ""),
            self.id,
            self.ssh_address.replace(['\n', '\r'], "")
        );
        let temporary = state_dir.join(format!("{}.tmp", crate::layout::GITHUB_LINK));
        std::fs::write(&temporary, text).map_err(failed)?;
        std::fs::rename(&temporary, state_dir.join(crate::layout::GITHUB_LINK)).map_err(failed)
    }

    pub(crate) fn load(state_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(state_dir.join(crate::layout::GITHUB_LINK)).ok()?;
        let value = |name: &str| text.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix('=')).map(str::to_owned);
        let full_name = value("repository")?;
        // The HTTPS path is built from it: GitHub's rules for names only.
        github_repository(&full_name)?;
        Some(Self { full_name, id: value("id")?.parse().ok()?, ssh_address: value("ssh")? })
    }

    /// Whether `store` (the saved address) is this repository: the address
    /// setup was given for it, or GitHub's SSH address of the same
    /// `owner/name` (port 22, or GitHub's port 443 host).
    fn is_store(&self, store: &StoreAddress) -> bool {
        is_repository(store, &self.full_name, &self.ssh_address)
    }
}

/// Whether `store` is the GitHub repository `full_name` (`owner/name`)
/// whose SSH address is `ssh_address`: that address, or GitHub's SSH
/// address of the same `owner/name` on either of its ports.
fn is_repository(store: &StoreAddress, full_name: &str, ssh_address: &str) -> bool {
    StoreAddress::parse(ssh_address).is_ok_and(|linked| linked.address == *store)
        || store.github_name().is_some_and(|name| name.eq_ignore_ascii_case(full_name))
}

/// Whether the store address `address` is the GitHub repository
/// `full_name` (`owner/name`) whose SSH address is `ssh_address`: what a
/// refresh with the sign-in checks before it sends anything.
#[uniffi::export]
pub fn is_github_repository(address: String, full_name: String, ssh_address: String) -> bool {
    StoreAddress::parse(&address).is_ok_and(|store| is_repository(&store.address, &full_name, &ssh_address))
}

/// The steps of a token refresh, for stopping between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RefreshStep {
    /// Getting a usable access token (may refresh it: network).
    Token,
    /// Fetching over HTTPS.
    Fetch,
    /// Checking what was fetched.
    Check,
    /// Integrating it into the store.
    Integrate,
}

/// Something only the person can decide, found by a background refresh.
/// Nothing was changed for it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RefreshDecision {
    /// A `.gpg-id` changed on the server; not integrated in the
    /// background. A foreground refresh brings it in and reports it.
    RecipientsChanged { changes: Vec<RecipientsChange> },
    /// The server's tree holds what paw will not check out.
    RefusedRemote { paths: Vec<RefusedPath> },
    /// The server's history was replaced.
    RemoteReplaced { replaced: Replaced },
    /// Waiting changes not encrypted for their entry's recipients now.
    HeldBack { count: u32 },
}

/// How a background refresh ended.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum BackgroundOutcome {
    /// No sign-in is kept: nothing was sent anywhere.
    NotSignedIn,
    /// No repository chosen for this store: nothing was sent.
    NotLinked,
    /// The store already had everything.
    UpToDate,
    /// New changes came in (see `integration`).
    Updated,
    /// Stopped for the person: see `decisions`.
    NeedsDecision,
    /// GitHub no longer accepts the sign-in; it was forgotten. Sign in again.
    SignedOut,
    /// GitHub could not be reached.
    Offline { message: String },
    /// The deadline came first; the store is as it was or fully updated.
    OutOfTime,
    /// Another part of paw (or the AutoFill extension) was using the store
    /// or fetching it. Nothing was changed; the next run tries again.
    Busy,
    Failed { message: String },
}

/// What a background refresh did, compactly.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BackgroundRefreshReport {
    pub outcome: BackgroundOutcome,
    /// What integrating brought in, when it ran.
    pub integration: Option<IntegrateReport>,
    pub decisions: Vec<RefreshDecision>,
    /// Local changes still waiting to be sent (with the key).
    pub unpushed: u32,
}

impl BackgroundRefreshReport {
    fn ended(outcome: BackgroundOutcome) -> Self {
        Self { outcome, integration: None, decisions: Vec::new(), unpushed: 0 }
    }
}

/// Why a token fetch failed.
pub(crate) enum FetchError {
    GitHub(GitHubError),
    Paw(PawError),
    /// GitHub did not accept the token (401).
    Unauthorised,
}

impl From<GitHubError> for FetchError {
    fn from(e: GitHubError) -> Self {
        Self::GitHub(e)
    }
}

impl From<FetchError> for SyncError {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::Paw(e) => e.into(),
            FetchError::GitHub(GitHubError::Network { message }) => SyncError::Network { message },
            FetchError::GitHub(other) => SyncError::Failed { message: other.to_string() },
            FetchError::Unauthorised => SyncError::Failed { message: still_refused().to_string() },
        }
    }
}

/// A token GitHub refused even after it was refreshed.
fn still_refused() -> GitHubError {
    GitHubError::Refused {
        message: "GitHub did not let paw's sign-in read this repository; is paw still installed on it?".into(),
    }
}

impl GitHubAccount {
    fn repo_dir(&self) -> PathBuf {
        crate::layout::repo_dir(&self.state_dir)
    }

    /// The chosen repository, if the store's remote is it. Reads files only.
    fn checked_link(&self) -> Result<GitHubLink, GitHubError> {
        let link = GitHubLink::load(&self.state_dir).ok_or(GitHubError::NotLinked)?;
        let store = saved_address(&self.state_dir)
            .map_err(|e| GitHubError::Failed { message: e.to_string() })?;
        if !link.is_store(&store) {
            return Err(GitHubError::WrongRepository { store: store.display(), granted: link.full_name });
        }
        Ok(link)
    }

    fn fetch_once(
        &self,
        http: &Http,
        link: &GitHubLink,
        token: Secret,
        fetch_wait: Duration,
        progress: Option<crate::sync::TransferCallback<'_>>,
    ) -> Result<Fetching, FetchError> {
        let repo = crate::sync::guard::open(&self.repo_dir()).map_err(FetchError::Paw)?;
        // The transfer runs without the store lock, under the fetch lock,
        // into private refs; [`Self::publish`] moves them.
        let fetching = crate::sync::begin_fetch_within(&self.store_lock(), &repo, fetch_wait).map_err(FetchError::Paw)?;
        let limit = *self.download_limit.lock().unwrap_or_else(|p| p.into_inner());
        let remote = HttpsRemote::new(http.clone(), &link.full_name, token, limit);
        match remote.with_remote(&repo, |r| crate::sync::fetch_connected(r, progress)) {
            Ok(_) => Ok(fetching),
            Err((error, failure)) => Err(match explain(error, failure) {
                Ok(paw) => FetchError::Paw(paw),
                Err(Failure::Unauthorised) => FetchError::Unauthorised,
                Err(Failure::GitHub(e)) => FetchError::GitHub(e),
                Err(Failure::TooBig(_)) => FetchError::Paw(PawError::Git("the git host sent too much".into())),
            }),
        }
    }

    fn store_lock(&self) -> StoreLock {
        StoreLock::for_state_dir(&self.state_dir)
    }

    /// With the store lock: what [`Self::fetch_with_token`] received
    /// becomes the store's `origin/*`. Returns the lock, held.
    fn publish(&self, fetching: Fetching, wait: Duration) -> Result<Exclusive, PawError> {
        let held = self.store_lock().exclusive_within(wait)?;
        let repo = crate::sync::guard::open(&self.repo_dir())?;
        crate::sync::publish_fetched(&held, &repo, fetching)?;
        Ok(held)
    }

    /// Fetches every branch of the chosen repository over HTTPS, without
    /// the store lock, into private refs for [`Self::publish`]. Changes
    /// nothing else.
    fn fetch_with_token(
        &self,
        http: &Http,
        link: &GitHubLink,
        fetch_wait: Duration,
        mut progress: Option<crate::sync::TransferCallback<'_>>,
    ) -> Result<Fetching, FetchError> {
        let fetch = |token| {
            let progress = progress.as_mut().map(|p| &mut **p as crate::sync::TransferCallback<'_>);
            self.fetch_once(http, link, token, fetch_wait, progress)
        };
        match self.with_token(http, fetch, |e| matches!(e, FetchError::Unauthorised)) {
            Err(FetchError::Unauthorised) => Err(FetchError::GitHub(still_refused())),
            other => other,
        }
    }

    /// The background refresh: see the module comment. Ends by `budget`
    /// from now. Never asks anyone anything, never pushes.
    pub fn background_refresh(&self, budget: Duration) -> BackgroundRefreshReport {
        self.background_refresh_with(budget, &|_| false)
    }

    /// [`Self::background_refresh`] that also stops before `stop(step)`
    /// says so, as if the deadline had come (or the app were killed) there.
    /// During the transfer `stop(RefreshStep::Fetch)` is asked at each
    /// chunk, so a stop from another thread (the background task's
    /// expiration) cuts it short; the store is left as it was.
    pub fn background_refresh_with(&self, budget: Duration, stop: &dyn Fn(RefreshStep) -> bool) -> BackgroundRefreshReport {
        let deadline = Instant::now() + budget;
        let stopping = |step| stop(step) || Instant::now() >= deadline;
        // How long to wait for a lock: briefly, and never past the deadline.
        let lock_wait = || BACKGROUND_LOCK_WAIT.min(deadline.saturating_duration_since(Instant::now()));
        let failed = |e: &dyn std::fmt::Display| BackgroundRefreshReport::ended(BackgroundOutcome::Failed { message: e.to_string() });
        match self.keeper.signed_in() {
            Ok(true) => {}
            Ok(false) => return BackgroundRefreshReport::ended(BackgroundOutcome::NotSignedIn),
            Err(e) => return failed(&e),
        }
        let link = match self.checked_link() {
            Ok(link) => link,
            Err(GitHubError::NotLinked) => return BackgroundRefreshReport::ended(BackgroundOutcome::NotLinked),
            Err(e) => return failed(&e),
        };
        let out_of_time = || BackgroundRefreshReport::ended(BackgroundOutcome::OutOfTime);
        let from_github = |e: GitHubError| match e {
            GitHubError::SignedOut => BackgroundRefreshReport::ended(BackgroundOutcome::SignedOut),
            GitHubError::NotSignedIn => BackgroundRefreshReport::ended(BackgroundOutcome::NotSignedIn),
            GitHubError::Network { message } if Instant::now() >= deadline => {
                let _ = message;
                out_of_time()
            }
            GitHubError::Network { message } => BackgroundRefreshReport::ended(BackgroundOutcome::Offline { message }),
            other => failed(&other),
        };

        if stopping(RefreshStep::Token) {
            return out_of_time();
        }
        let http = Http::new(&self.endpoints, Some(deadline));
        // Refreshed here if need be, so the fetch below starts with a good one.
        if let Err(e) = self.keeper.access_token(&http, None) {
            return from_github(e);
        }
        if stopping(RefreshStep::Fetch) {
            return out_of_time();
        }
        let mut in_time = |_: TransferProgress| !stopping(RefreshStep::Fetch);
        let fetching = match self.fetch_with_token(&http, &link, lock_wait(), Some(&mut in_time)) {
            Ok(fetching) => fetching,
            Err(_) if stopping(RefreshStep::Fetch) => return out_of_time(),
            Err(FetchError::Paw(PawError::StoreBusy)) => return BackgroundRefreshReport::ended(BackgroundOutcome::Busy),
            Err(FetchError::GitHub(e)) => return from_github(e),
            Err(FetchError::Paw(PawError::Network(message))) => {
                return BackgroundRefreshReport::ended(BackgroundOutcome::Offline { message });
            }
            Err(FetchError::Paw(e)) => return failed(&e),
            Err(FetchError::Unauthorised) => return failed(&still_refused()),
        };
        // From here to the end, local work only, with the store lock held:
        // published, checked and integrated as one step.
        // Stopped after the transfer: nothing published, nothing changed.
        if stopping(RefreshStep::Check) {
            return out_of_time();
        }
        let held = match self.publish(fetching, lock_wait()) {
            Ok(held) => held,
            Err(PawError::StoreBusy) => return BackgroundRefreshReport::ended(BackgroundOutcome::Busy),
            Err(e) => return failed(&e),
        };

        if stopping(RefreshStep::Check) {
            return out_of_time();
        }
        let dir = self.repo_dir();
        let mut decisions = Vec::new();
        match crate::sync::refused_remote(&dir) {
            Ok(paths) if !paths.is_empty() => decisions.push(RefreshDecision::RefusedRemote { paths }),
            Ok(_) => {}
            Err(e) => return failed(&e),
        }
        match crate::sync::integrate::pending_recipients_changes(&dir) {
            Ok(changes) if !changes.is_empty() => decisions.push(RefreshDecision::RecipientsChanged { changes }),
            Ok(_) => {}
            Err(e) => return failed(&e),
        }
        let Waiting { held_back, unpushed } = Waiting::count(&dir, &self.state_dir);
        if held_back > 0 {
            decisions.push(RefreshDecision::HeldBack { count: held_back });
        }
        let parked = decisions.iter().any(|d| !matches!(d, RefreshDecision::HeldBack { .. }));
        if parked {
            return BackgroundRefreshReport { outcome: BackgroundOutcome::NeedsDecision, integration: None, decisions, unpushed };
        }

        if stopping(RefreshStep::Integrate) {
            return out_of_time();
        }
        let report = match crate::sync::operation::integrate_fetched(&held, &dir) {
            Ok(report) => report,
            Err(e) => return failed(&e),
        };
        let integration = report.integration;
        let unpushed = match report.state {
            SyncState::RemoteReplaced { replaced } => {
                decisions.push(RefreshDecision::RemoteReplaced { replaced });
                return BackgroundRefreshReport { outcome: BackgroundOutcome::NeedsDecision, integration: None, decisions, unpushed };
            }
            SyncState::Waiting { unpushed } => unpushed,
            SyncState::UpToDate => 0,
        };
        let changed = integration.fast_forwarded > 0 || integration.replayed > 0;
        let outcome = if !decisions.is_empty() {
            BackgroundOutcome::NeedsDecision
        } else if changed {
            BackgroundOutcome::Updated
        } else {
            BackgroundOutcome::UpToDate
        };
        BackgroundRefreshReport { outcome, integration: Some(integration), decisions, unpushed }
    }
}

/// What is waiting to be sent, for the background report. The counts only
/// inform the person; one that can't be read now counts as none, so the run
/// still brings in what was fetched.
struct Waiting {
    held_back: u32,
    unpushed: u32,
}

impl Waiting {
    fn count(dir: &Path, state_dir: &Path) -> Self {
        Self {
            held_back: crate::sync::queue::held_back(dir, state_dir).map_or(0, |held| held.len() as u32),
            unpushed: crate::sync::queue::unpushed(dir).map_or(0, |queue| queue.len() as u32),
        }
    }
}

/// Stops a `GitHubAccount.background_refresh_stoppable` from another thread.
#[derive(uniffi::Object, Default)]
pub struct BackgroundStop {
    stopped: AtomicBool,
}

#[uniffi::export]
impl BackgroundStop {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The refresh ends at its next step or chunk. Returns at once.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

#[uniffi::export]
impl GitHubAccount {
    /// Fetches over HTTPS with the sign-in and integrates exactly as
    /// `StoreSync.refresh` does, pushing nothing. No key, no card. Refused
    /// before anything is sent if there is no sign-in, no chosen
    /// repository, or the store's remote is another repository. Sign-in
    /// problems come as `Failed` (or `Network`); `is_signed_in` then says
    /// whether the person must sign in again.
    pub fn refresh_store(&self) -> Result<SyncReport, SyncError> {
        if !self.keeper.signed_in().map_err(FetchError::GitHub)? {
            return Err(FetchError::GitHub(GitHubError::NotSignedIn).into());
        }
        let link = self.checked_link().map_err(FetchError::GitHub)?;
        let http = Http::new(&self.endpoints, None);
        let fetching = self.fetch_with_token(&http, &link, crate::lock::FETCH_WAIT, None)?;
        let held = self.publish(fetching, crate::lock::DEFAULT_WAIT)?;
        crate::sync::operation::integrate_fetched(&held, &self.repo_dir())
    }

    /// The background refresh: at most `seconds` long, never asks anyone
    /// anything, never pushes, and leaves every decision to the person
    /// (see `BackgroundRefreshReport.decisions`). Safe to be killed. Ends
    /// early once `stop.stop()` is called from any thread (the background
    /// task's expiration handler): between steps, or at the transfer's
    /// next chunk; a request already waiting on the network ends by the
    /// deadline. Ends `OutOfTime` then, with the store as it was or fully
    /// updated, and no lock held.
    pub fn background_refresh_stoppable(&self, seconds: u32, stop: Arc<BackgroundStop>) -> BackgroundRefreshReport {
        self.background_refresh_with(Duration::from_secs(u64::from(seconds)), &|_| stop.is_stopped())
    }
}

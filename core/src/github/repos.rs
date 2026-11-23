//! The repositories the person granted paw's GitHub App, for the setup
//! picker.
//!
//! `GET /user/installations` lists where the app is installed that the
//! person can see; `GET /user/installations/{id}/repositories` lists each
//! installation's repositories the person can see. Both are paged (100 a
//! page) to the end, within caps. Choosing one gives the SSH address setup
//! already takes; the token is never used to push.
//!
//! Whether a repository looks like a pass store: one
//! `GET /repos/{owner}/{repo}/contents/.gpg-id` each (on the default
//! branch), for at most [`MAX_STORE_PROBES`] repositories, most recently
//! pushed first. It needs no permission beyond the read access to contents
//! the fetch already needs, and a 404 costs GitHub nothing. Past the cap
//! the answer is "not checked" (`None`), not "no".

use serde::Deserialize;

use super::http::Http;
use super::{Endpoints, GitHubError, Secret};

/// The most repositories checked for a `.gpg-id`: one request each.
pub const MAX_STORE_PROBES: usize = 20;
/// One page of a list.
const PER_PAGE: u32 = 100;
/// The most installation pages read.
const MAX_INSTALLATION_PAGES: u32 = 10;
/// The most repository pages read per installation (5,000 repositories).
const MAX_REPOSITORY_PAGES: u32 = 50;

/// A repository the person granted paw, as the picker shows it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GitHubRepository {
    /// GitHub's id; stays the same if the repository is renamed.
    pub id: u64,
    /// `owner/name`.
    pub full_name: String,
    pub owner: String,
    pub name: String,
    pub private: bool,
    pub default_branch: String,
    /// What setup is given when this is picked, e.g.
    /// `git@github.com:you/password-store.git`.
    pub ssh_address: String,
    /// When something was last pushed, as GitHub writes it
    /// (`2026-11-20T09:12:00Z`), if ever.
    pub pushed_at: Option<String>,
    /// `Some(true)` if it has a `.gpg-id` at the top of its default
    /// branch, `Some(false)` if not, `None` if not checked.
    pub looks_like_store: Option<bool>,
    /// The installation it was granted through.
    pub installation_id: u64,
}

/// What the picker shows.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RepositoryList {
    /// The app is not installed anywhere the person can see, or no
    /// repository is granted: "Install paw on the repository" with
    /// `install_url`, then list again.
    NotInstalled { install_url: String },
    /// The granted repositories, likely stores first, then most recently
    /// pushed. `install_url` adds more.
    Repositories { repositories: Vec<GitHubRepository>, install_url: String },
}

#[derive(Deserialize)]
struct Installations {
    total_count: Option<u64>,
    installations: Vec<Installation>,
}

#[derive(Deserialize)]
struct Installation {
    id: u64,
    app_slug: Option<String>,
}

#[derive(Deserialize)]
struct Repositories {
    total_count: Option<u64>,
    repositories: Vec<Repository>,
}

#[derive(Deserialize)]
struct Owner {
    login: String,
}

#[derive(Deserialize)]
struct Repository {
    id: u64,
    name: String,
    owner: Owner,
    #[serde(default)]
    private: bool,
    default_branch: Option<String>,
    pushed_at: Option<String>,
}

/// What a 401 is until the token has been refreshed and tried again: see
/// [`is_unauthorised`].
pub(crate) const UNAUTHORISED: &str = "GitHub did not accept paw's sign-in";

pub(crate) fn unauthorised() -> GitHubError {
    GitHubError::Refused { message: UNAUTHORISED.into() }
}

/// Whether GitHub refused the access token (401), so a refreshed one is
/// worth one more try.
pub(crate) fn is_unauthorised(error: &GitHubError) -> bool {
    matches!(error, GitHubError::Refused { message } if message == UNAUTHORISED)
}

fn unreadable() -> GitHubError {
    GitHubError::Refused { message: "GitHub's list was not readable".into() }
}

/// What a REST answer's status means, if not 200.
pub(crate) fn api_status(status: u16) -> Result<(), GitHubError> {
    match status {
        200 => Ok(()),
        401 => Err(unauthorised()),
        500.. => Err(GitHubError::Network { message: format!("GitHub answered {status}") }),
        other => Err(GitHubError::Refused { message: format!("GitHub answered {other}") }),
    }
}

/// Reads one page with `get`, which returns the status and the body.
fn page<T: for<'de> Deserialize<'de>>(http: &Http, token: &Secret, path: &str) -> Result<T, GitHubError> {
    let answer = http.api_get(path, token)?;
    api_status(answer.status)?;
    serde_json::from_slice(&answer.body).map_err(|_| unreadable())
}

/// A date GitHub wrote, kept only if it looks like one.
fn plausible_date(text: Option<String>) -> Option<String> {
    text.filter(|t| t.len() <= 40 && t.chars().all(|c| c.is_ascii_alphanumeric() || "-:+.".contains(c)))
}

/// A branch name, kept only if it is plain enough to show.
fn plausible_branch(text: Option<String>) -> String {
    text.filter(|t| !t.is_empty() && t.len() <= 255 && !t.chars().any(char::is_control))
        .unwrap_or_else(|| "main".into())
}

/// Lists every repository granted, with `token`. Repositories whose names
/// paw would not accept as an address are left out.
pub(crate) fn list(http: &Http, token: &Secret) -> Result<RepositoryList, GitHubError> {
    let endpoints: &Endpoints = http.endpoints();
    let install_url = endpoints.install_url();
    let mut installations = Vec::new();
    for number in 1..=MAX_INSTALLATION_PAGES {
        let found: Installations = page(http, token, &format!("/user/installations?per_page={PER_PAGE}&page={number}"))?;
        let count = found.installations.len();
        installations.extend(
            found
                .installations
                .into_iter()
                .filter(|i| i.app_slug.as_deref().is_none_or(|slug| slug == endpoints.app_slug())),
        );
        let total = found.total_count.unwrap_or(0) as usize;
        if count < PER_PAGE as usize || installations.len() >= total {
            break;
        }
    }

    let mut repositories: Vec<GitHubRepository> = Vec::new();
    for installation in &installations {
        let mut seen = 0usize;
        for number in 1..=MAX_REPOSITORY_PAGES {
            let path = format!("/user/installations/{}/repositories?per_page={PER_PAGE}&page={number}", installation.id);
            let found: Repositories = page(http, token, &path)?;
            let count = found.repositories.len();
            seen += count;
            for repository in found.repositories {
                if repositories.iter().any(|r| r.id == repository.id) {
                    continue;
                }
                let owner = repository.owner.login;
                let full_name = format!("{owner}/{}", repository.name);
                // GitHub's own rules for the names; anything else is not
                // offered, since it would not be accepted as an address.
                let Some(ssh_address) = endpoints.ssh_address(&full_name) else {
                    continue;
                };
                repositories.push(GitHubRepository {
                    id: repository.id,
                    full_name,
                    owner,
                    name: repository.name,
                    private: repository.private,
                    default_branch: plausible_branch(repository.default_branch),
                    ssh_address,
                    pushed_at: plausible_date(repository.pushed_at),
                    looks_like_store: None,
                    installation_id: installation.id,
                });
            }
            let total = found.total_count.unwrap_or(0) as usize;
            if count < PER_PAGE as usize || seen >= total {
                break;
            }
        }
    }
    if repositories.is_empty() {
        return Ok(RepositoryList::NotInstalled { install_url });
    }

    repositories.sort_by(|a, b| b.pushed_at.cmp(&a.pushed_at).then_with(|| a.full_name.cmp(&b.full_name)));
    for repository in repositories.iter_mut().take(MAX_STORE_PROBES) {
        repository.looks_like_store = probe_store(http, token, &repository.full_name)?;
    }
    // Likely stores first; the order within each group stays.
    repositories.sort_by_key(|r| match r.looks_like_store {
        Some(true) => 0,
        None => 1,
        Some(false) => 2,
    });
    Ok(RepositoryList::Repositories { repositories, install_url })
}

/// Whether `full_name` has a `.gpg-id` at the top of its default branch.
fn probe_store(http: &Http, token: &Secret, full_name: &str) -> Result<Option<bool>, GitHubError> {
    let answer = http.api_get(&format!("/repos/{full_name}/contents/.gpg-id"), token)?;
    Ok(match answer.status {
        200 => Some(true),
        404 => Some(false),
        401 => return Err(unauthorised()),
        _ => None,
    })
}

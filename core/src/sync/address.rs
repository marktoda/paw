//! The address of a password store, as a person types or pastes it.
//!
//! paw speaks git over SSH only. It accepts the two forms git hosts show:
//! `git@github.com:owner/repo.git` and `ssh://git@host[:port]/path`, and
//! three short ways of naming a GitHub repository, each turned into its SSH
//! address with a notice saying so: `owner/repo`, `github.com/owner/repo`
//! and the repository's web address `https://github.com/owner/repo`. Spaces
//! around the address and a slash at its end are ignored.
//!
//! Everything else is refused with a sentence that says what to do instead,
//! in particular anything that could reach `ssh` or `git` as an option (a
//! user, host or path starting with `-`), control characters, a password or
//! token inside the address (never repeated back), and git's other
//! transports (`file://`, `ext::`, `git://`).

use std::path::Path;

use super::hosts::{GITHUB_SSH, is_github};
use crate::PawError;

/// Where a store lives, parsed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StoreAddress {
    pub user: String,
    pub host: String,
    pub port: u16,
    /// As the host expects it after `git-upload-pack`: `owner/repo.git` for
    /// the short form, `/owner/repo.git` for an `ssh://` address.
    pub path: String,
}

/// The result of reading what the person typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAddress {
    pub address: StoreAddress,
    /// Set when paw changed the address, saying how, for the person to see.
    pub notice: Option<String>,
}

const NOT_GIT: &str = "That doesn't look like a git address. It looks like git@github.com:you/password-store.git.";
const USE_SSH: &str = "paw connects over SSH for now. Use the address that starts with git@ or ssh://.";

fn valid_host(host: &str) -> bool {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return !inner.is_empty() && inner.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
    }
    !host.is_empty()
        && !host.starts_with(['-', '.'])
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

fn valid_user(user: &str) -> bool {
    !user.is_empty()
        && !user.starts_with('-')
        && user.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

fn valid_path(path: &str) -> bool {
    let bare = path.trim_start_matches('/');
    !bare.is_empty() && !path.starts_with('-') && !path.chars().any(char::is_control)
}

/// Never repeats what was pasted: it may be a live password or token.
const HAS_SECRET: &str = "That address has a user name, password or token in it. paw logs in with your YubiKey and needs none of them: remove that part. If it was a password or token, change it, since it has been copied.";

/// git's `<transport>::<address>` form (`ext::`, `fd::`, ...), which runs
/// whatever the transport says.
fn is_remote_helper(text: &str) -> bool {
    text.split_once("::").is_some_and(|(scheme, _)| {
        !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    })
}

/// A GitHub account name: letters, digits and single hyphens inside.
fn github_owner(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 39
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A GitHub repository name: letters, digits, `.`, `_` and `-`; never `.`
/// or `..`, and not starting with `-`.
fn github_repo(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// The SSH address of the GitHub repository `owner/repo`, and the notice
/// that says paw read it that way.
fn github(owner: &str, repo: &str, what: &str) -> Result<ParsedAddress, String> {
    let address = github_address(owner, repo).ok_or_else(|| NOT_GIT.to_owned())?;
    let notice = format!("{what} paw connects over SSH, so it will use {} instead.", address.display());
    Ok(ParsedAddress { address, notice: Some(notice) })
}

/// The SSH address of the GitHub repository `owner/repo` (with or without
/// `.git`), if those are names GitHub allows.
fn github_address(owner: &str, repo: &str) -> Option<StoreAddress> {
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    (github_owner(owner) && github_repo(repo)).then(|| StoreAddress {
        user: "git".into(),
        host: GITHUB_SSH.0.into(),
        port: GITHUB_SSH.1,
        path: format!("{owner}/{repo}.git"),
    })
}

/// The SSH address of the GitHub repository named `full_name`
/// (`owner/name`, as GitHub's API gives it), if it is such a name.
pub(crate) fn github_repository(full_name: &str) -> Option<StoreAddress> {
    let (owner, repo) = full_name.split_once('/')?;
    github_address(owner, repo)
}

/// `owner/repo` or `github.com/owner/repo` (with or without `.git`), or
/// `None` if the text is neither.
fn github_short_form(text: &str) -> Result<Option<ParsedAddress>, String> {
    if text.contains([':', '@', '\\']) || text.starts_with('/') {
        return Ok(None);
    }
    let parts: Vec<&str> = text.split('/').collect();
    match parts.as_slice() {
        [owner, repo] if !owner.contains('.') => {
            github(owner, repo, &format!("That was GitHub's short form for {owner}/{repo}.")).map(Some)
        }
        [host, owner, repo] if ["github.com", "www.github.com"].contains(&host.to_ascii_lowercase().as_str()) => {
            github(owner, repo, "That was a GitHub address without its user name.").map(Some)
        }
        [host, ..] if host.contains('.') && valid_host(host) => Err(format!(
            "paw needs the SSH address, as in git@{host}:you/password-store.git. Your git host shows it beside the Clone button."
        )),
        _ => Ok(None),
    }
}

impl StoreAddress {
    /// Reads an address as typed or pasted. Spaces around it are ignored.
    pub fn parse(text: &str) -> Result<ParsedAddress, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Type or paste the address of your password store.".into());
        }
        if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("An address has no spaces in it. Check what was pasted.".into());
        }
        // `git@host:you/store/` and `https://github.com/you/store/` name
        // the same repository as without the slash.
        let text = text.trim_end_matches('/');
        if text.is_empty() {
            return Err(NOT_GIT.into());
        }
        let lower = text.to_ascii_lowercase();
        if lower.starts_with("https://") || lower.starts_with("http://") {
            return from_web(text);
        }
        if let Some(rest) = lower.strip_prefix("ssh://").map(|_| &text["ssh://".len()..]) {
            return from_ssh_url(rest).map(|address| ParsedAddress { address, notice: None });
        }
        if text.contains("://") || is_remote_helper(text) {
            return Err(USE_SSH.into());
        }
        if let Some(parsed) = github_short_form(text)? {
            return Ok(parsed);
        }
        from_scp(text).map(|address| ParsedAddress { address, notice: None })
    }

    /// The address in the form shown to people and saved: the short form
    /// when it can be written that way, else `ssh://`.
    pub fn display(&self) -> String {
        if self.port == 22 && !self.path.starts_with('/') && !self.host.starts_with('[') {
            format!("{}@{}:{}", self.user, self.host, self.path)
        } else {
            let port = if self.port == 22 { String::new() } else { format!(":{}", self.port) };
            let separator = if self.path.starts_with('/') { "" } else { "/~/" };
            format!("ssh://{}@{}{port}{separator}{}", self.user, self.host, self.path)
        }
    }

    /// The host to connect to: without the brackets of an IPv6 address.
    pub fn connect_host(&self) -> &str {
        self.host.trim_start_matches('[').trim_end_matches(']')
    }

    /// Whether this is a repository on GitHub's SSH endpoint (either
    /// port), and which: `owner/name` as written in the address.
    pub(crate) fn github_name(&self) -> Option<&str> {
        if !is_github(self.connect_host(), self.port) {
            return None;
        }
        let path = self.path.trim_start_matches('/');
        Some(path.strip_suffix(".git").unwrap_or(path))
    }
}

/// `user@host:path`.
fn from_scp(text: &str) -> Result<StoreAddress, String> {
    // `user:password@host:path`.
    if text.split_once('@').is_some_and(|(before, _)| before.contains(':')) {
        return Err(HAS_SECRET.into());
    }
    let Some((login, path)) = text.split_once(':') else {
        return Err(NOT_GIT.into());
    };
    let Some((user, host)) = login.split_once('@') else {
        return Err(if login.contains('/') || login.contains('\\') {
            "That looks like a folder on a computer. paw needs the address of a git server.".into()
        } else {
            "Add the user name in front, as in git@github.com:you/password-store.git.".into()
        });
    };
    if !valid_user(user) || !valid_host(host) {
        return Err(NOT_GIT.into());
    }
    if !valid_path(path) {
        return Err("Add the repository after the colon, as in git@github.com:you/password-store.git.".into());
    }
    Ok(StoreAddress { user: user.into(), host: host.into(), port: 22, path: path.into() })
}

/// What follows `ssh://`: `user@host[:port]/path`.
fn from_ssh_url(rest: &str) -> Result<StoreAddress, String> {
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let Some((user, host_port)) = authority.rsplit_once('@') else {
        return Err("Add the user name, as in ssh://git@example.com/you/password-store.git.".into());
    };
    if user.contains(':') {
        return Err(HAS_SECRET.into());
    }
    let (host, port) = if let Some(end) = host_port.find(']') {
        let (host, after) = host_port.split_at(end + 1);
        (host, after.strip_prefix(':'))
    } else {
        match host_port.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        }
    };
    let port = match port {
        None => 22,
        Some(port) => match port.parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => return Err("The port after the colon must be a number from 1 to 65535.".into()),
        },
    };
    if !valid_user(user) || !valid_host(host) {
        return Err(NOT_GIT.into());
    }
    // `ssh://host/~/repo` means a path relative to the home folder.
    let path = path.strip_prefix("/~/").unwrap_or(path);
    if !valid_path(path) {
        return Err("Add the repository's path after the host, as in ssh://git@example.com/you/password-store.git.".into());
    }
    Ok(StoreAddress { user: user.into(), host: host.into(), port, path: path.into() })
}

/// A web address. Only a GitHub repository's page has a known SSH address.
fn from_web(text: &str) -> Result<ParsedAddress, String> {
    let rest = text.split_once("://").map(|(_, rest)| rest).unwrap_or_default();
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    // `https://name:token@github.com/...`, or a token as the user name.
    if host.contains('@') {
        return Err(HAS_SECRET.into());
    }
    let host = host.to_ascii_lowercase();
    if host != "github.com" && host != "www.github.com" {
        return Err(USE_SSH.into());
    }
    let parts: Vec<&str> = path.split('/').collect();
    let [owner, repo] = parts.as_slice() else {
        return Err("Use the address of the repository itself, as in https://github.com/you/password-store, or its SSH address.".into());
    };
    github(owner, repo, "That was a web address.")
}

/// The store address a finished download saved in `state_dir`.
pub fn saved_address(state_dir: &Path) -> Result<StoreAddress, PawError> {
    let path = state_dir.join(crate::files::layout::REMOTE);
    let text = crate::files::read_bounded(&path, crate::files::MAX_FILE_BYTES)
        .and_then(|found| found.ok_or_else(|| std::io::ErrorKind::NotFound.into()))
        .and_then(|bytes| String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
        .map_err(|e| PawError::Store(format!("no store is set up here: {e}")))?;
    StoreAddress::parse(&text).map(|parsed| parsed.address).map_err(PawError::Store)
}

/// The host a store address connects to, for sentences ("github.com");
/// empty if it can't be read as an address.
#[uniffi::export]
pub fn store_address_host(address: String) -> String {
    StoreAddress::parse(&address).map(|parsed| parsed.address.connect_host().to_owned()).unwrap_or_default()
}

/// Whether a store address is a repository on GitHub.
#[uniffi::export]
pub fn is_github_address(address: String) -> bool {
    StoreAddress::parse(&address).is_ok_and(|parsed| parsed.address.github_name().is_some())
}

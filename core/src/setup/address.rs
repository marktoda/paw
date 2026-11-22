//! The address of a password store, as a person types or pastes it.
//!
//! paw speaks git over SSH only. It accepts the two forms git hosts show:
//! `git@github.com:owner/repo.git` and `ssh://git@host[:port]/path`. A web
//! address of a GitHub repository is turned into its SSH address, and the
//! person is told. Everything else is refused with a sentence that says
//! what to do instead.

use crate::sync::{HostKeyPin, Remote};

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
        let lower = text.to_ascii_lowercase();
        if lower.starts_with("https://") || lower.starts_with("http://") {
            return from_web(text);
        }
        if let Some(rest) = lower.strip_prefix("ssh://").map(|_| &text["ssh://".len()..]) {
            return from_ssh_url(rest).map(|address| ParsedAddress { address, notice: None });
        }
        if text.contains("://") {
            return Err(USE_SSH.into());
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

    /// The remote to connect to, with the host keys it must present.
    pub fn remote(&self, host_key: HostKeyPin) -> Remote {
        Remote {
            host: self.connect_host().to_owned(),
            port: self.port,
            user: self.user.clone(),
            path: self.path.clone(),
            host_key,
        }
    }
}

/// `user@host:path`.
fn from_scp(text: &str) -> Result<StoreAddress, String> {
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
    let host = host.to_ascii_lowercase();
    if host != "github.com" && host != "www.github.com" {
        return Err(USE_SSH.into());
    }
    let path = path.trim_end_matches('/');
    let parts: Vec<&str> = path.split('/').collect();
    let [owner, repo] = parts.as_slice() else {
        return Err("Use the address of the repository itself, as in https://github.com/you/password-store, or its SSH address.".into());
    };
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let fine = |part: &str| {
        !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    };
    if !fine(owner) || !fine(repo) {
        return Err(NOT_GIT.into());
    }
    let address = StoreAddress {
        user: "git".into(),
        host: "github.com".into(),
        port: 22,
        path: format!("{owner}/{repo}.git"),
    };
    let notice = format!(
        "That was a web address. paw connects over SSH, so it will use {} instead.",
        address.display()
    );
    Ok(ParsedAddress { address, notice: Some(notice) })
}

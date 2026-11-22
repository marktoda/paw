//! Which host keys paw trusts.
//!
//! github.com's keys ship with paw. Any other host is trusted on first use,
//! and only after the person has seen its fingerprint and confirmed it; the
//! confirmed key is then pinned in a known-hosts file next to the store. A
//! host that later presents a different key is refused, whatever the
//! reason: the only way past it is to forget the pinned key on purpose.

use std::path::{Path, PathBuf};

use super::transport::HostKeyPin;
use crate::PawError;

/// github.com's SSH host keys, ed25519 first.
///
/// Taken on 2026-11-22 from GitHub's documentation page "GitHub's SSH key
/// fingerprints"
/// (<https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints>)
/// and cross-checked against the `ssh_keys` and `ssh_key_fingerprints` of
/// <https://api.github.com/meta> fetched the same day. Both gave these keys
/// and these fingerprints. The RSA key is pinned for completeness, but this
/// build has no RSA, so it is never offered (see `HostKeyPin::algorithms`).
pub const GITHUB_HOST_KEYS: [(&str, &str); 3] = [
    (
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
        "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU",
    ),
    (
        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEmKSENjQEezOmxkZMy7opKgwFB9nkt5YRrYMjNuG5N87uRgg6CLrbo5wAdT/y6v0mKV0U2w0WZ2YB/++Tpockg=",
        "SHA256:p2QAMXNIC1TJYWeIOttrVc98/R1BUFWu3/LiyKgUfQM",
    ),
    (
        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCj7ndNxQowgcQnjshcLrqPEiiphnt+VTTvDP6mHBL9j1aNUkY4Ue1gvwnGLVlOhGeYrnZaMgRK6+PKCUXaDbC7qtbW8gIkhL7aGCsOr/C56SJMy/BCZfxd1nWzAOxSDPgVsmerOBYfNqltV9/hWCqBywINIR+5dIg6JTJ72pcEpEjcYgXkE2YEFXV1JHnsKgbLWNlhScqb2UmyRkQyytRLtL+38TGxkxCflmO+5Z8CSSNY7GidjMIZ7Q4zMjA2n1nGrlTDkzwDCsw+wqFPGQA179cnfGWOWRVruj16z6XyvxvjJwbz0wQZ75XK5tKSb7FNyeIEs4TT4jk+S4dhPeAUC5y+bDYirYgM4GC7uEnztnZyaVWQ7B381AK4Qdrwt51ZqExKbQpTUNn+EjqoTwvqNj4kqx5QUCI0ThS/YkOxJCXmPUWZbhjpCg56i+2aB6CmK2JGhn57K5mj0MNdBXA4/WnwH6XoPWJzK5Nyu2zB3nAZp+S5hpQs+p1vN1/wsjk=",
        "SHA256:uNiVztksCsDhcc0u9e8BujQXVUpKZIDTMczCvj3tD2s",
    ),
];

/// The keys that ship with paw for `host`, if any.
pub fn bundled(host: &str, port: u16) -> Option<HostKeyPin> {
    if !host.eq_ignore_ascii_case("github.com") || port != 22 {
        return None;
    }
    let lines: Vec<&str> = GITHUB_HOST_KEYS.iter().map(|(line, _)| *line).collect();
    HostKeyPin::parse_all(&lines).ok()
}

/// Host keys confirmed by the person, one host per line:
/// `host port algorithm base64`. Lives in the store's state directory.
pub struct KnownHosts {
    path: PathBuf,
}

fn io_err(e: std::io::Error) -> PawError {
    PawError::Store(format!("could not read or write the known hosts: {e}"))
}

impl KnownHosts {
    pub fn new(path: &Path) -> Self {
        Self { path: path.to_path_buf() }
    }

    fn read(&self) -> Result<Vec<(String, u16, String)>, PawError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(e)),
        };
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(3, ' ');
                let host = fields.next()?.to_owned();
                let port = fields.next()?.parse().ok()?;
                let key = fields.next()?.to_owned();
                Some((host, port, key))
            })
            .collect())
    }

    fn write(&self, entries: &[(String, u16, String)]) -> Result<(), PawError> {
        let text: String = entries
            .iter()
            .map(|(host, port, key)| format!("{host} {port} {key}\n"))
            .collect();
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, text).map_err(io_err)?;
        std::fs::rename(&temporary, &self.path).map_err(io_err)
    }

    /// The pinned key for `host`, if one was confirmed.
    pub fn lookup(&self, host: &str, port: u16) -> Result<Option<HostKeyPin>, PawError> {
        let lines: Vec<String> = self
            .read()?
            .into_iter()
            .filter(|(h, p, _)| h.eq_ignore_ascii_case(host) && *p == port)
            .map(|(_, _, key)| key)
            .collect();
        if lines.is_empty() {
            return Ok(None);
        }
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        HostKeyPin::parse_all(&lines).map(Some)
    }

    /// Pins `key_line` for `host`, replacing nothing: a host that already
    /// has a pinned key must be forgotten first.
    pub fn add(&self, host: &str, port: u16, key_line: &str) -> Result<(), PawError> {
        if key_line.contains(['\n', '\r']) || host.contains([' ', '\n', '\r']) {
            return Err(PawError::Store("not a host key line".into()));
        }
        let mut entries = self.read()?;
        if entries.iter().any(|(h, p, _)| h.eq_ignore_ascii_case(host) && *p == port) {
            return Err(PawError::Store(format!("{host} already has a saved key")));
        }
        entries.push((host.to_owned(), port, key_line.to_owned()));
        self.write(&entries)
    }

    /// Forgets the key pinned for `host`: Settings › Store › Forget server key.
    pub fn forget(&self, host: &str, port: u16) -> Result<(), PawError> {
        let mut entries = self.read()?;
        entries.retain(|(h, p, _)| !(h.eq_ignore_ascii_case(host) && *p == port));
        self.write(&entries)
    }
}

/// The pin for `host`: the bundled keys, else a confirmed one, else `None`
/// (the host must be confirmed first).
pub fn pin_for(known: &KnownHosts, host: &str, port: u16) -> Result<Option<HostKeyPin>, PawError> {
    match bundled(host, port) {
        Some(pin) => Ok(Some(pin)),
        None => known.lookup(host, port),
    }
}

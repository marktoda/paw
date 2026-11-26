//! Which host keys paw trusts.
//!
//! The keys of a few large git hosts ship with paw ([`BUNDLED`]). Any other
//! host is trusted on first use, and only after the person has seen its
//! fingerprint and confirmed it; the confirmed key is then pinned in a
//! known-hosts file next to the store. A host that later presents a
//! different key is refused, whatever the reason: the only way past it is to
//! forget the pinned key on purpose, and a bundled host's keys cannot be
//! forgotten at all.
//!
//! Every bundled key was checked on 2026-11-23 against the host's own
//! documentation, fetched over HTTPS; each list names its page. The SHA256
//! fingerprint beside each key is the one that page publishes, and a test
//! checks that each key has it. Hosts not here because their documentation
//! publishes fingerprints but not the keys themselves: codeberg.org
//! (<https://docs.codeberg.org/security/ssh-fingerprint/>) and git.sr.ht.
//! They are confirmed on first use like any other host.

use std::path::{Path, PathBuf};

use super::address::StoreAddress;
use super::transport::{HostKeyPin, PresentedHostKey, Remote};
use crate::PawError;

/// github.com's SSH host keys, ed25519 first, with their fingerprints.
///
/// From GitHub's documentation page "GitHub's SSH key fingerprints"
/// (<https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints>),
/// checked on 2026-11-22 and again on 2026-11-23 against the `ssh_keys` and
/// `ssh_key_fingerprints` of <https://api.github.com/meta>. All gave these
/// keys and these fingerprints. The RSA key is pinned for completeness, but
/// this build has no RSA, so it is never offered (see
/// `HostKeyPin::algorithms`).
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

/// gitlab.com's SSH host keys, ed25519 first, with their fingerprints.
///
/// The keys are the "SSH `known_hosts` entries" and the fingerprints the
/// "SSH host keys fingerprints" of GitLab's documentation page "GitLab.com
/// settings" (<https://docs.gitlab.com/user/gitlab_com/>), fetched on
/// 2026-11-23.
pub const GITLAB_HOST_KEYS: [(&str, &str); 3] = [
    (
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAfuCHKVTjquxvt6CM6tdG4SLp1Btn/nOeHHE5UOzRdf",
        "SHA256:eUXGGm1YGsMAS7vkcx6JOJdOGHPem5gQp4taiCfCLB8",
    ),
    (
        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBFSMqzJeV9rUzU4kWitGjeR4PWSa29SPqJ1fVkhtj3Hw9xjLVXVYrU9QlYWrOLXBpQ6KWjbjTDTdDkoohFzgbEY=",
        "SHA256:HbW3g8zUjNSksFbqTiUWPWg2Bq1x8xdGUrliXFzSnUw",
    ),
    (
        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCsj2bNKTBSpIYDEGk9KxsGh3mySTRgMtXL583qmBpzeQ+jqCMRgBqB98u3z++J1sKlXHWfM9dyhSevkMwSbhoR8XIq/U0tCNyokEi/ueaBMCvbcTHhO7FcwzY92WK4Yt0aGROY5qX2UKSeOvuP4D6TPqKF1onrSzH9bx9XUf2lEdWT/ia1NEKjunUqu1xOB/StKDHMoX4/OKyIzuS0q/T1zOATthvasJFoPrAjkohTyaDUz2LN5JoH839hViyEG82yB+MjcFV5MU3N1l1QL3cVUCh93xSaua1N85qivl+siMkPGbO5xR/En4iEY6K2XPASUEMaieWVNTRCtJ4S8H+9",
        "SHA256:ROQFvPThGrW4RuWLoL9tq9I9zJ42fK4XywyRtbOz/EQ",
    ),
];

/// bitbucket.org's SSH host keys, ed25519 first, with their fingerprints.
///
/// The fingerprints are the "public SSH host key fingerprints" of
/// Atlassian's documentation page "Configure SSH and two-step verification"
/// (<https://support.atlassian.com/bitbucket-cloud/docs/configure-ssh-and-two-step-verification/>),
/// and the keys are what that page says to fetch for a known-hosts file,
/// <https://bitbucket.org/site/ssh>; both fetched on 2026-11-23, and each key
/// has the fingerprint the page gives for it.
pub const BITBUCKET_HOST_KEYS: [(&str, &str); 3] = [
    (
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIIazEu89wgQZ4bqs3d63QSMzYVa0MuJ2e2gKTKqu+UUO",
        "SHA256:ybgmFkzwOSotHTHLJgHO0QN8L0xErw6vd0VhFA9m3SM",
    ),
    (
        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBPIQmuzMBuKdWeF4+a2sjSSpBK0iqitSQ+5BM9KhpexuGt20JpTVM7u5BDZngncgrqDMbWdxMWWOGtZ9UgbqgZE=",
        "SHA256:FC73VB6C4OQLSCrjEayhMp9UMxS97caD/Yyi2bhW/J0",
    ),
    (
        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQDQeJzhupRu0u0cdegZIa8e86EG2qOCsIsD1Xw0xSeiPDlCr7kq97NLmMbpKTX6Esc30NuoqEEHCuc7yWtwp8dI76EEEB1VqY9QJq6vk+aySyboD5QF61I/1WeTwu+deCbgKMGbUijeXhtfbxSxm6JwGrXrhBdofTsbKRUsrN1WoNgUa8uqN1Vx6WAJw1JHPhglEGGHea6QICwJOAr/6mrui/oB7pkaWKHj3z7d1IC4KWLtY47elvjbaTlkN04Kc/5LFEirorGYVbt15kAUlqGM65pk6ZBxtaO3+30LVlORZkxOh+LKL/BvbZ/iRNhItLqNyieoQj/uh/7Iv4uyH/cV/0b4WDSd3DptigWq84lJubb9t/DnZlrJazxyDCulTmKdOR7vs9gMTo+uoIrPSb8ScTtvw65+odKAlBj59dhnVp9zd7QUojOpXlL62Aw56U4oO+FALuevvMjiWeavKhJqlR7i5n9srYcrNV7ttmDw7kf/97P5zauIhxcjX+xHv4M=",
        "SHA256:46OSHA1Rmj8E8ERTC6xkNcmGOw9oFxYr0WF6zWW8l1E",
    ),
];

/// A host's bundled keys: each public key line with the SHA256 fingerprint
/// its documentation gives for it.
pub type HostKeys = &'static [(&'static str, &'static str)];

/// GitHub's SSH endpoint, and the same host on the HTTPS port for networks
/// that block port 22. GitHub's page "Using SSH over the HTTPS port"
/// (<https://docs.github.com/en/authentication/troubleshooting-ssh/using-ssh-over-the-https-port>,
/// fetched 2026-11-23) shows the second presenting the ed25519 fingerprint
/// above and says to accept it if it matches one of GitHub's published
/// fingerprints: it has the same keys.
pub(crate) const GITHUB_SSH: (&str, u16) = ("github.com", 22);
const GITHUB_SSH_443: (&str, u16) = ("ssh.github.com", 443);

/// The host and port each set of keys is bundled for.
pub const BUNDLED: [(&str, u16, HostKeys); 4] = [
    (GITHUB_SSH.0, GITHUB_SSH.1, &GITHUB_HOST_KEYS),
    (GITHUB_SSH_443.0, GITHUB_SSH_443.1, &GITHUB_HOST_KEYS),
    ("gitlab.com", 22, &GITLAB_HOST_KEYS),
    ("bitbucket.org", 22, &BITBUCKET_HOST_KEYS),
];

fn is(endpoint: (&str, u16), host: &str, port: u16) -> bool {
    host.eq_ignore_ascii_case(endpoint.0) && port == endpoint.1
}

/// Whether `host` on `port` is GitHub's SSH endpoint, on either port.
pub(crate) fn is_github(host: &str, port: u16) -> bool {
    is(GITHUB_SSH, host, port) || is(GITHUB_SSH_443, host, port)
}

/// Another way in to a host whose usual port a network may block, with the
/// same bundled keys: for github.com on port 22, ssh.github.com on 443.
pub(crate) fn port_fallback(host: &str, port: u16) -> Option<(&'static str, u16)> {
    is(GITHUB_SSH, host, port).then_some(GITHUB_SSH_443)
}

/// The keys that ship with paw for `host` on `port`, if any.
pub fn bundled(host: &str, port: u16) -> Option<HostKeyPin> {
    let (_, _, keys) = BUNDLED.iter().find(|(h, p, _)| is((h, *p), host, port))?;
    let lines: Vec<&str> = keys.iter().map(|(line, _)| *line).collect();
    HostKeyPin::parse_all(&lines).ok()
}

/// Whether paw ships keys for `host` on `port`: such a host is never
/// confirmed by the person, and its keys cannot be forgotten.
pub fn is_bundled(host: &str, port: u16) -> bool {
    BUNDLED.iter().any(|(h, p, _)| is((h, *p), host, port))
}

/// Why a bundled host's key is not forgotten or replaced.
fn bundled_refusal(host: &str) -> PawError {
    PawError::HostKeyBundled { host: host.to_owned() }
}

/// Host keys confirmed by the person, one host per line:
/// `host port algorithm base64`, in the store's state directory.
pub struct KnownHosts {
    state_dir: PathBuf,
}

fn io_err(e: std::io::Error) -> PawError {
    PawError::Store(format!("could not read or write the known hosts: {e}"))
}

impl KnownHosts {
    /// The known hosts of the store kept in `state_dir`.
    pub fn of(state_dir: &Path) -> Self {
        Self { state_dir: state_dir.to_path_buf() }
    }

    fn path(&self) -> PathBuf {
        self.state_dir.join(crate::layout::KNOWN_HOSTS)
    }

    /// The store's lock: a change reads, changes and writes the whole file.
    fn lock(&self) -> Result<crate::lock::Exclusive, PawError> {
        crate::lock::StoreLock::for_state_dir(&self.state_dir).exclusive()
    }

    fn read(&self) -> Result<Vec<(String, u16, String)>, PawError> {
        let text = match std::fs::read_to_string(self.path()) {
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
        let path = self.path();
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, text).map_err(io_err)?;
        std::fs::rename(&temporary, &path).map_err(io_err)
    }

    /// The confirmed key for `host`, if one was confirmed.
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

    /// The keys `host` must present: the bundled ones, else a confirmed
    /// one, else `None` (the host must be confirmed first).
    pub fn pin(&self, host: &str, port: u16) -> Result<Option<HostKeyPin>, PawError> {
        match bundled(host, port) {
            Some(pin) => Ok(Some(pin)),
            None => self.lookup(host, port),
        }
    }

    /// The remote at `address`, held to its host's keys; `None` while the
    /// host is not confirmed.
    pub fn remote(&self, address: &StoreAddress) -> Result<Option<Remote>, PawError> {
        let pin = self.pin(address.connect_host(), address.port)?;
        Ok(pin.map(|host_key| Remote {
            host: address.connect_host().to_owned(),
            port: address.port,
            user: address.user.clone(),
            path: address.path.clone(),
            host_key,
            route: Default::default(),
        }))
    }

    /// Pins `key_line` for `host`, replacing nothing: a host that already
    /// has a pinned key must be forgotten first. Refused for a host whose
    /// keys ship with paw.
    pub fn add(&self, host: &str, port: u16, key_line: &str) -> Result<(), PawError> {
        if is_bundled(host, port) {
            return Err(bundled_refusal(host));
        }
        if key_line.contains(['\n', '\r']) || host.contains([' ', '\n', '\r']) {
            return Err(PawError::Store("not a host key line".into()));
        }
        let _held = self.lock()?;
        let mut entries = self.read()?;
        if entries.iter().any(|(h, p, _)| h.eq_ignore_ascii_case(host) && *p == port) {
            return Err(PawError::Store(format!("{host} already has a saved key")));
        }
        entries.push((host.to_owned(), port, key_line.to_owned()));
        self.write(&entries)
    }

    /// The person compared the fingerprint `checked` and trusts it: pins
    /// the key `presented` gives, if it is the one with that fingerprint.
    /// `Ok(false)`, pinning nothing, if it is another. Refused, before
    /// `presented` is asked, for a host whose keys ship with paw.
    pub fn trust(
        &self,
        host: &str,
        port: u16,
        presented: impl FnOnce() -> Result<PresentedHostKey, PawError>,
        checked: &str,
    ) -> Result<bool, PawError> {
        if is_bundled(host, port) {
            return Err(bundled_refusal(host));
        }
        let presented = presented()?;
        if presented.fingerprint() != checked.trim() {
            return Ok(false);
        }
        self.add(host, port, &presented.line()?)?;
        Ok(true)
    }

    /// Forgets the key pinned for `host`: Settings › Store › Forget server
    /// key, or "I expected this change" in setup. Refused for a host whose
    /// keys ship with paw: a different key from one of them is always
    /// refused.
    pub fn forget(&self, host: &str, port: u16) -> Result<(), PawError> {
        if is_bundled(host, port) {
            return Err(bundled_refusal(host));
        }
        let _held = self.lock()?;
        let mut entries = self.read()?;
        entries.retain(|(h, p, _)| !(h.eq_ignore_ascii_case(host) && *p == port));
        self.write(&entries)
    }
}

//! paw's only HTTP client: blocking HTTP/1.1 over rustls (ring, with the
//! Mozilla roots compiled in), used for GitHub alone.
//!
//! - Certificates are always verified against the web PKI. Nothing is
//!   pinned beyond that: GitHub rotates its certificates and their issuers,
//!   and a pin would break every copy of paw when it does. Instead the host
//!   is restricted: credentials are attached only to a URL whose scheme,
//!   host and port are exactly GitHub's ([`Http::may_carry_credentials`]).
//! - Redirects are never followed by the client. The git fetch follows one
//!   itself only to the same origin (`https://github.com`), so a token
//!   never goes anywhere else.
//! - No proxy from the environment, no compression, `User-Agent: paw`.
//! - Every request ends by a deadline, and every response body is capped.

use std::io::Read;
use std::time::{Duration, Instant};

use ureq::Agent;
use ureq::http::{Response, Uri};
use zeroize::Zeroizing;

use super::{API_VERSION, Endpoints, GitHubError, Secret};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const GIT_TIMEOUT: Duration = Duration::from_secs(300);
/// The most a sign-in or token answer may be: a few hundred bytes in fact.
pub(crate) const FORM_LIMIT: u64 = 64 << 10;
/// The most one API page may be: 100 repositories are about 600 KiB.
pub(crate) const API_LIMIT: u64 = 8 << 20;
pub(crate) const MAX_REDIRECTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Host {
    Web,
    Api,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

fn origin_of(url: &str) -> Option<Origin> {
    let uri: Uri = url.parse().ok()?;
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let host = uri.host()?.to_ascii_lowercase();
    // A user or password in the URL is never ours to send.
    if uri.authority()?.as_str().contains('@') {
        return None;
    }
    let port = match (uri.port_u16(), scheme.as_str()) {
        (Some(port), _) => port,
        (None, "https") => 443,
        (None, "http") => 80,
        _ => return None,
    };
    Some(Origin { scheme, host, port })
}

/// A request that went wrong before an answer could be read.
pub(crate) fn network(error: ureq::Error) -> GitHubError {
    let message = match error {
        ureq::Error::Timeout(_) => "it took too long".to_owned(),
        ureq::Error::HostNotFound => "no such host; is the iPhone online?".to_owned(),
        ureq::Error::ConnectionFailed => "could not connect".to_owned(),
        ureq::Error::BodyExceedsLimit(limit) => format!("the answer was larger than {limit} bytes"),
        ureq::Error::Tls(e) => format!("the secure connection failed: {e}"),
        ureq::Error::Io(e) => e.to_string(),
        // Built from paw's own URLs and GitHub's answer, never a token:
        // tokens are only ever sent in headers and bodies.
        other => other.to_string(),
    };
    GitHubError::network(message)
}

pub(crate) struct Answer {
    pub status: u16,
    /// Wiped when dropped: a token answer holds tokens.
    pub body: Zeroizing<Vec<u8>>,
}

/// The HTTP client for one operation, which must end by `deadline`.
#[derive(Clone)]
pub(crate) struct Http {
    agent: Agent,
    endpoints: Endpoints,
    deadline: Option<Instant>,
}

impl Http {
    pub(crate) fn new(endpoints: &Endpoints, deadline: Option<Instant>) -> Self {
        let config = Agent::config_builder()
            // Status codes are answers paw reads, not errors.
            .http_status_as_error(false)
            // Plain HTTP only to a test build's simulated GitHub.
            .https_only(!endpoints.is_simulated())
            .max_redirects(0)
            .max_redirects_will_error(false)
            .proxy(None)
            .user_agent("paw")
            .accept_encoding("identity")
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .max_idle_connections(0)
            .build();
        Self { agent: config.into(), endpoints: endpoints.clone(), deadline }
    }

    pub(crate) fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    pub(crate) fn time_left(&self, cap: Duration) -> Result<Duration, GitHubError> {
        match self.deadline {
            None => Ok(cap),
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    Err(GitHubError::network("out of time"))
                } else {
                    Ok(left.min(cap))
                }
            }
        }
    }

    fn base(&self, host: Host) -> &str {
        match host {
            Host::Web => self.endpoints.web(),
            Host::Api => self.endpoints.api(),
        }
    }

    /// Whether a token may be attached to a request for `url`: its scheme,
    /// host and port are exactly those of GitHub's `host` (on a device
    /// build: `https`, `github.com` or `api.github.com`, port 443), and it
    /// names no user or password of its own. Every request that carries a
    /// token or the device code is checked here first.
    pub(crate) fn may_carry_credentials(&self, url: &str, host: Host) -> bool {
        let (Some(target), Some(base)) = (origin_of(url), origin_of(self.base(host))) else {
            return false;
        };
        if target != base {
            return false;
        }
        if self.endpoints.is_simulated() {
            return true;
        }
        let expected = match host {
            Host::Web => "github.com",
            Host::Api => "api.github.com",
        };
        target.scheme == "https" && target.host == expected && target.port == 443
    }

    fn refuse_target() -> GitHubError {
        GitHubError::refused("paw only sends its GitHub sign-in to github.com")
    }

    /// The form may hold a token; it is wiped after sending.
    pub(crate) fn post_form(&self, path: &str, form: &[(&str, &str)]) -> Result<Answer, GitHubError> {
        let url = format!("{}{path}", self.endpoints.web());
        if !self.may_carry_credentials(&url, Host::Web) {
            return Err(Self::refuse_target());
        }
        let mut body = Zeroizing::new(String::new());
        for (index, (name, value)) in form.iter().enumerate() {
            if index > 0 {
                body.push('&');
            }
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let response = self
            .agent
            .post(&url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .config()
            .timeout_global(Some(self.time_left(REQUEST_TIMEOUT)?))
            .build()
            .send(body.as_bytes())
            .map_err(network)?;
        read_answer(response, FORM_LIMIT)
    }

    pub(crate) fn api_get(&self, path_and_query: &str, token: &Secret) -> Result<Answer, GitHubError> {
        let url = format!("{}{path_and_query}", self.endpoints.api());
        if !self.may_carry_credentials(&url, Host::Api) || !token.well_formed() {
            return Err(Self::refuse_target());
        }
        let authorization = Zeroizing::new(format!("Bearer {}", token.expose()));
        let response = self
            .agent
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header("Authorization", authorization.as_str())
            .config()
            .timeout_global(Some(self.time_left(REQUEST_TIMEOUT)?))
            .build()
            .call()
            .map_err(network)?;
        read_answer(response, API_LIMIT)
    }

    /// POSTs `json` to `path` on the REST API with no credentials at all.
    pub(crate) fn api_post_anonymous(&self, path: &str, json: &[u8]) -> Result<Answer, GitHubError> {
        let url = format!("{}{path}", self.endpoints.api());
        if !self.may_carry_credentials(&url, Host::Api) {
            return Err(Self::refuse_target());
        }
        let response = self
            .agent
            .post(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header("Content-Type", "application/json")
            .config()
            .timeout_global(Some(self.time_left(REQUEST_TIMEOUT)?))
            .build()
            .send(json)
            .map_err(network)?;
        read_answer(response, FORM_LIMIT)
    }

    /// One git smart-HTTP request with the token as basic auth (user
    /// `x-access-token`), to `url` on github.com only. A GET follows at
    /// most [`MAX_REDIRECTS`] redirects, each to github.com again (checked
    /// before the token is attached); a redirect anywhere else is refused
    /// and nothing is sent there. A POST follows none.
    pub(crate) fn git(
        &self,
        url: &str,
        token: &Secret,
        post: Option<(&str, &[u8])>,
    ) -> Result<Response<ureq::Body>, GitHubError> {
        let mut url = url.to_owned();
        let mut hops = 0;
        loop {
            if !self.may_carry_credentials(&url, Host::Web) || !token.well_formed() {
                return Err(if hops == 0 {
                    Self::refuse_target()
                } else {
                    GitHubError::refused("GitHub sent paw to another address; paw did not follow it")
                });
            }
            let basic = Zeroizing::new(format!("x-access-token:{}", token.expose()));
            let authorization = Zeroizing::new(format!("Basic {}", base64_encode(basic.as_bytes())));
            let timeout = Some(self.time_left(GIT_TIMEOUT)?);
            let response = match post {
                None => self
                    .agent
                    .get(&url)
                    .header("Authorization", authorization.as_str())
                    .header("Git-Protocol", "version=0")
                    .config()
                    .timeout_global(timeout)
                    .build()
                    .call(),
                Some((content_type, body)) => self
                    .agent
                    .post(&url)
                    .header("Authorization", authorization.as_str())
                    .header("Content-Type", content_type)
                    .header("Accept", "application/x-git-upload-pack-result")
                    .config()
                    .timeout_global(timeout)
                    .build()
                    .send(body),
            }
            .map_err(network)?;
            let status = response.status().as_u16();
            if !(300..400).contains(&status) {
                return Ok(response);
            }
            if post.is_some() || hops >= MAX_REDIRECTS {
                return Err(GitHubError::refused(format!("GitHub redirected the fetch ({status})")));
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| GitHubError::refused("GitHub redirected the fetch nowhere"))?;
            url = if location.starts_with('/') && !location.starts_with("//") {
                format!("{}{location}", self.endpoints.web())
            } else {
                location.to_owned()
            };
            hops += 1;
        }
    }
}

fn read_answer(response: Response<ureq::Body>, limit: u64) -> Result<Answer, GitHubError> {
    let status = response.status().as_u16();
    let mut reader = response.into_body().into_with_config().limit(limit).reader();
    let mut body = Zeroizing::new(Vec::new());
    reader
        .read_to_end(&mut body)
        .map_err(|e| GitHubError::network(format!("the answer was cut short or too large: {e}")))?;
    Ok(Answer { status, body })
}

/// `application/x-www-form-urlencoded` for one name or value.
fn form_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_githubs_exact_origin_may_carry_a_token() {
        let http = Http::new(&Endpoints::github(), None);
        assert!(http.may_carry_credentials("https://github.com/you/store.git/info/refs", Host::Web));
        assert!(http.may_carry_credentials("https://GitHub.com:443/you/store.git", Host::Web));
        assert!(http.may_carry_credentials("https://api.github.com/user/installations", Host::Api));
        for url in [
            "http://github.com/you/store.git",
            "https://github.com.evil.example/you/store.git",
            "https://evil.example/github.com/you/store.git",
            "https://gist.github.com/you/store.git",
            "https://github.com:8443/you/store.git",
            "https://user@github.com/you/store.git",
            "https://api.github.com/you/store.git",
            "//github.com/you/store.git",
            "github.com/you/store.git",
        ] {
            assert!(!http.may_carry_credentials(url, Host::Web), "{url}");
        }
        assert!(!http.may_carry_credentials("https://github.com/user", Host::Api));
    }

    #[test]
    fn forms_are_encoded() {
        assert_eq!(form_encode("urn:ietf:params:oauth:grant-type:device_code"), "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code");
        assert_eq!(form_encode("a b&c=d"), "a%20b%26c%3Dd");
    }

    #[test]
    fn a_secret_shows_nothing_of_itself() {
        let secret = Secret::new("ghu_abcdef".into());
        assert_eq!(format!("{secret:?}"), "Secret(..)");
        assert!(secret.well_formed());
        assert!(!Secret::new("ghu_abc\r\nX-Evil: 1".into()).well_formed());
        assert!(!Secret::new(String::new()).well_formed());
    }
}

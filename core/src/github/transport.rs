//! Fetching from GitHub over HTTPS with the access token: a libgit2 smart
//! subtransport over [`Http`], built like the SSH one in
//! `crate::sync::transport`.
//!
//! libgit2 is built without HTTPS of its own. This transport speaks git's
//! smart HTTP protocol (versions 0 and 1, stateless): `GET
//! <repo>/info/refs?service=git-upload-pack` for the refs, then one `POST
//! <repo>/git-upload-pack` per negotiation round, the last of which
//! carries the pack. `git-receive-pack` is refused before anything is
//! sent: the token never pushes.
//!
//! What the host sends is counted as it arrives against the same cap as
//! SSH ([`DOWNLOAD_LIMIT`] unless lowered), over the whole fetch, and the
//! transfer stops past it before libgit2 writes the pack.

use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};

use git2::transport::{Service, SmartSubtransport, SmartSubtransportStream, Transport};

use super::http::Http;
use super::{GitHubError, Secret};
use crate::PawError;
use crate::remote::RemoteFailure;
use crate::sync::{DOWNLOAD_LIMIT, SyncError};

/// The URL libgit2 is given; the real one is in [`ACTIVE`].
const TRANSPORT_URL: &str = "paw-https://github";
/// The most one negotiation request may be: wants and haves are a few KiB.
const REQUEST_LIMIT: usize = 1 << 20;

/// Why a token fetch failed, before it is turned into an error.
#[derive(Debug, Clone)]
pub(crate) enum Failure {
    /// More than the cap arrived.
    TooBig(u64),
    /// A GitHub-level failure (refused token, refused redirect, not found,
    /// network).
    GitHub(GitHubError),
}

struct Shared {
    http: Http,
    /// `<web>/<owner>/<repo>.git`.
    repo_url: String,
    token: Secret,
    limit: u64,
    received: AtomicU64,
    failure: Mutex<Option<Failure>>,
}

impl Shared {
    fn fail(&self, failure: Failure) -> io::Error {
        let message = match &failure {
            Failure::TooBig(_) => "the git host sent too much".to_owned(),
            Failure::GitHub(e) => e.to_string(),
        };
        let mut slot = self.failure.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_none() {
            *slot = Some(failure);
        }
        io::Error::other(message)
    }

    fn git_error(&self, failure: Failure) -> git2::Error {
        git2::Error::from_str(&self.fail(failure).to_string())
    }
}

/// One fetch's HTTPS connection to `owner/repo` on GitHub.
pub(crate) struct HttpsRemote {
    shared: Arc<Shared>,
}

impl HttpsRemote {
    pub(crate) fn new(http: Http, full_name: &str, token: Secret, limit: Option<u64>) -> Self {
        let repo_url = format!("{}/{full_name}.git", http.endpoints().web());
        Self {
            shared: Arc::new(Shared {
                http,
                repo_url,
                token,
                limit: limit.unwrap_or(DOWNLOAD_LIMIT),
                received: AtomicU64::new(0),
                failure: Mutex::new(None),
            }),
        }
    }

    /// Runs `operation` against a libgit2 remote whose transport is this
    /// connection.
    pub(crate) fn with_remote<T>(
        &self,
        repo: &git2::Repository,
        operation: impl FnOnce(&mut git2::Remote<'_>) -> Result<T, git2::Error>,
    ) -> Result<T, SyncError> {
        register_transport();
        ACTIVE.with(|active| *active.borrow_mut() = Some(self.shared.clone()));
        let result = repo
            .remote_anonymous(TRANSPORT_URL)
            .and_then(|mut remote| operation(&mut remote));
        ACTIVE.with(|active| *active.borrow_mut() = None);
        result.map_err(|e| explain(e, self.shared.failure.lock().unwrap_or_else(|p| p.into_inner()).take()))
    }
}

/// A failed fetch as paw reports it: what went wrong in the transport, if
/// it was the transport.
fn explain(error: git2::Error, failure: Option<Failure>) -> SyncError {
    match failure {
        Some(Failure::TooBig(limit)) => PawError::Git(format!(
            "the git host sent more than {} MiB, far more than a password store; paw stopped the download",
            limit >> 20
        ))
        .into(),
        Some(Failure::GitHub(error)) => error.into(),
        None => PawError::Git(error.message().to_owned()).into(),
    }
}

thread_local! {
    /// The connection the libgit2 call on this thread uses; see the SSH
    /// transport for why a thread-local.
    static ACTIVE: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) };
}

fn register_transport() {
    static REGISTERED: Once = Once::new();
    REGISTERED.call_once(|| {
        let factory = |remote: &git2::Remote<'_>| {
            let shared = ACTIVE
                .with(|active| active.borrow().clone())
                .ok_or_else(|| git2::Error::from_str("no GitHub connection is open for this call"))?;
            // `true`: stateless, one request per round, as HTTP is.
            Transport::smart(remote, true, Subtransport(shared))
        };
        // SAFETY: as for the SSH transport: `Once` serialises registration,
        // and every transport of this scheme is created by `with_remote`,
        // which calls this first.
        if let Err(e) = unsafe { git2::transport::register("paw-https", factory) } {
            // Only possible if the scheme were registered twice, which
            // `Once` prevents; a fetch then fails with "unsupported URL".
            debug_assert!(false, "registering the HTTPS transport: {e}");
        }
    });
}

struct Subtransport(Arc<Shared>);

impl SmartSubtransport for Subtransport {
    fn action(&self, _url: &str, service: Service) -> Result<Box<dyn SmartSubtransportStream>, git2::Error> {
        let shared = self.0.clone();
        match service {
            Service::UploadPackLs => {
                let url = format!("{}/info/refs?service=git-upload-pack", shared.repo_url);
                let body = request(&shared, &url, None, "application/x-git-upload-pack-advertisement")?;
                Ok(Box::new(Stream { shared, request: None, body: Some(body) }))
            }
            Service::UploadPack => Ok(Box::new(Stream { shared, request: Some(Vec::new()), body: None })),
            Service::ReceivePackLs | Service::ReceivePack => {
                Err(git2::Error::from_str("paw never pushes with its GitHub sign-in"))
            }
        }
    }

    fn close(&self) -> Result<(), git2::Error> {
        Ok(())
    }
}

type Body = Box<dyn Read + Send>;

/// Sends one request and checks the answer is what git expects.
fn request(shared: &Arc<Shared>, url: &str, post: Option<&[u8]>, expected_type: &str) -> Result<Body, git2::Error> {
    let post = post.map(|body| ("application/x-git-upload-pack-request", body));
    let response = shared.http.git(url, &shared.token, post).map_err(|e| shared.git_error(Failure::GitHub(e)))?;
    let status = response.status().as_u16();
    match status {
        200 => {}
        401 | 403 => return Err(shared.git_error(Failure::GitHub(RemoteFailure::Unauthorised.into()))),
        404 => {
            let message = "GitHub says the repository is not there, or paw's access to it was removed";
            return Err(shared.git_error(Failure::GitHub(GitHubError::refused(message))));
        }
        500.. => {
            let message = format!("GitHub answered {status}");
            return Err(shared.git_error(Failure::GitHub(GitHubError::network(message))));
        }
        other => {
            let message = format!("GitHub answered {other}");
            return Err(shared.git_error(Failure::GitHub(GitHubError::refused(message))));
        }
    }
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !content_type.starts_with(expected_type) {
        // A "dumb" server, or a login page: nothing paw speaks.
        let message = "GitHub did not answer as a git server";
        return Err(shared.git_error(Failure::GitHub(GitHubError::refused(message))));
    }
    // ureq stops at the cap too; paw's own count below names it.
    let reader = response.into_body().into_with_config().limit(shared.limit.saturating_add(1)).reader();
    Ok(Box::new(reader))
}

/// One request's body to send, then its answer to read.
struct Stream {
    shared: Arc<Shared>,
    /// The negotiation request being written, until the first read sends it.
    request: Option<Vec<u8>>,
    body: Option<Body>,
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.body.is_none() {
            let request = self.request.take().unwrap_or_default();
            let url = format!("{}/git-upload-pack", self.shared.repo_url);
            let body = request_io(&self.shared, &url, &request)?;
            self.body = Some(body);
        }
        let Some(body) = self.body.as_mut() else {
            return Ok(0);
        };
        let n = body.read(buf).map_err(|e| {
            if self.shared.received.load(Ordering::SeqCst) >= self.shared.limit {
                self.shared.fail(Failure::TooBig(self.shared.limit))
            } else {
                self.shared.fail(Failure::GitHub(GitHubError::network(e.to_string())))
            }
        })?;
        let total = self.shared.received.fetch_add(n as u64, Ordering::SeqCst) + n as u64;
        if total > self.shared.limit {
            return Err(self.shared.fail(Failure::TooBig(self.shared.limit)));
        }
        Ok(n)
    }
}

fn request_io(shared: &Arc<Shared>, url: &str, body: &[u8]) -> io::Result<Body> {
    request(shared, url, Some(body), "application/x-git-upload-pack-result").map_err(|e| io::Error::other(e.message().to_owned()))
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(request) = self.request.as_mut() else {
            return Err(io::Error::other("nothing more may be sent on this request"));
        };
        if request.len() + buf.len() > REQUEST_LIMIT {
            return Err(io::Error::other("the fetch request grew too large"));
        }
        request.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::Endpoints;

    #[test]
    fn the_https_transport_refuses_to_push_before_sending_anything() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let tree = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree).unwrap();
        let who = git2::Signature::now("t", "t@example.com").unwrap();
        repo.commit(Some("refs/heads/main"), &who, &who, "m", &tree, &[]).unwrap();
        // GitHub itself, but nothing may be sent: the push is refused first.
        let http = Http::new(&Endpoints::github(), Some(std::time::Instant::now()));
        let remote = HttpsRemote::new(http, "you/store", Secret::new("ghu_test".into()), None);
        let pushed = remote.with_remote(&repo, |r| r.push(&["refs/heads/main:refs/heads/main"], None));
        let Err(SyncError::Fault { fault: crate::fault::Fault::Local { detail } }) = pushed else { panic!("{pushed:?}") };
        assert!(detail.contains("never pushes"), "{detail}");
    }
}

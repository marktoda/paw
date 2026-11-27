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
//! What the host sends over the whole fetch is counted against the same
//! cap as SSH (`git_transport::DOWNLOAD_LIMIT` unless lowered), and the
//! transfer stops past it before libgit2 writes the pack.

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

use git2::transport::{Service, SmartSubtransportStream};

use super::http::Http;
use super::{GitHubError, Secret};
use crate::PawError;
use crate::git_transport::{self, Carrier, DOWNLOAD_LIMIT, DownloadCap};
use crate::remote::RemoteFailure;

/// The most one negotiation request may be: wants and haves are a few KiB.
const REQUEST_LIMIT: usize = 1 << 20;

struct Shared {
    http: Http,
    repo_url: String,
    token: Secret,
    cap: DownloadCap,
    /// The first failure of GitHub's, which libgit2 reports only as text.
    failure: Mutex<Option<GitHubError>>,
}

impl Shared {
    fn fail(&self, failure: GitHubError) -> io::Error {
        let message = failure.to_string();
        let mut slot = self.failure.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_none() {
            *slot = Some(failure);
        }
        io::Error::other(message)
    }

    fn git_error(&self, failure: GitHubError) -> git2::Error {
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
                cap: DownloadCap::new(limit.unwrap_or(DOWNLOAD_LIMIT)),
                failure: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn with_remote<T>(
        &self,
        repo: &git2::Repository,
        operation: impl FnOnce(&mut git2::Remote<'_>) -> Result<T, git2::Error>,
    ) -> Result<T, GitHubError> {
        git_transport::with_remote(repo, self.shared.clone(), operation, |e| {
            let failure = self.shared.failure.lock().unwrap_or_else(|p| p.into_inner()).take();
            failure.unwrap_or_else(|| PawError::Git(e.message().to_owned()).into())
        })
    }
}

impl Carrier for Shared {
    fn stateless(&self) -> bool {
        // One request per round, as HTTP is.
        true
    }

    fn open(self: Arc<Self>, service: Service) -> Result<Box<dyn SmartSubtransportStream>, git2::Error> {
        match service {
            Service::UploadPackLs => {
                let url = format!("{}/info/refs?service=git-upload-pack", self.repo_url);
                let body = request(&self, &url, None, "application/x-git-upload-pack-advertisement")?;
                Ok(Box::new(Stream { shared: self, request: None, body: Some(body) }))
            }
            Service::UploadPack => Ok(Box::new(Stream { shared: self, request: Some(Vec::new()), body: None })),
            Service::ReceivePackLs | Service::ReceivePack => {
                Err(git2::Error::from_str("paw never pushes with its GitHub sign-in"))
            }
        }
    }

    fn cap(&self) -> &DownloadCap {
        &self.cap
    }
}

type Body = Box<dyn Read + Send>;

fn request(shared: &Shared, url: &str, post: Option<&[u8]>, expected_type: &str) -> Result<Body, git2::Error> {
    let post = post.map(|body| ("application/x-git-upload-pack-request", body));
    let response = shared.http.git(url, &shared.token, post).map_err(|e| shared.git_error(e))?;
    let status = response.status().as_u16();
    match status {
        200 => {}
        401 | 403 => return Err(shared.git_error(RemoteFailure::Unauthorised.into())),
        404 => {
            let message = "GitHub says the repository is not there, or paw's access to it was removed";
            return Err(shared.git_error(GitHubError::refused(message)));
        }
        other => return Err(shared.git_error(GitHubError::answered(other))),
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
        return Err(shared.git_error(GitHubError::refused(message)));
    }
    // ureq stops past the cap too; paw's own count below stops first and
    // names it.
    let reader = response.into_body().into_with_config().limit(shared.cap.limit().saturating_add(1)).reader();
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
        let n = body.read(buf).map_err(|e| self.shared.fail(self.shared.http.cut_off(e.to_string())))?;
        self.shared.cap.count(n)?;
        Ok(n)
    }
}

fn request_io(shared: &Shared, url: &str, body: &[u8]) -> io::Result<Body> {
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
        let Err(GitHubError::Fault { fault: crate::fault::Fault::Local { detail } }) = pushed else { panic!("{pushed:?}") };
        assert!(detail.contains("never pushes"), "{detail}");
    }
}

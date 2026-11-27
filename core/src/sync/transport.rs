//! One authenticated SSH connection to the git host, and the glue that lets
//! libgit2 speak the git protocol over it.
//!
//! libgit2 is built without an SSH transport of its own, because its SSH
//! library cannot ask a card for a signature. Instead russh makes the
//! connection, with the login signed by an [`SshSigner`], and each git
//! service (`git-upload-pack`, `git-receive-pack`) runs in its own channel on
//! that connection. The login is the only thing that costs a touch, so a
//! fetch followed by a push on one [`Connection`] costs one.
//!
//! russh is async and libgit2 is blocking. The connection owns a small tokio
//! runtime whose worker thread keeps the SSH session moving, and the calling
//! thread blocks on it. None of this may be called from inside another async
//! runtime.

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use git2::transport::{Service, SmartSubtransportStream};
use russh::client::{self, AuthResult};
use russh::keys::agent::AgentIdentity;
use russh::keys::{Algorithm, HashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg, Disconnect};

use crate::fault::Fault;
use crate::git_transport::{self, Carrier, DOWNLOAD_LIMIT, DownloadCap};
use crate::PawError;
use crate::key::KeyError;
use crate::remote::RemoteFailure;
use crate::key::{SignFailure, SshSigner};
use crate::ssh::SshPublicKey;

/// How long to wait for the TCP connection and SSH handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// A connection that carries nothing for this long is treated as dead, so a
/// phone that lost its network does not hang in the middle of a fetch.
const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(60);
const STDERR_LIMIT: usize = 2048;

fn no_connection(host: &str, port: u16, why: &str) -> Fault {
    let fallback = super::hosts::port_fallback(host, port);
    RemoteFailure::NoConnection { host: host.to_owned(), detail: format!("could not connect: {why}"), fallback }.into()
}

fn unreachable(host: &str, detail: impl ToString) -> Fault {
    RemoteFailure::Unreachable { host: host.to_owned(), detail: detail.to_string() }.into()
}

/// Why dialling a host failed.
enum Dial<E> {
    /// No TCP connection, or no SSH handshake in time: why.
    NoConnection(String),
    /// The SSH side failed (the handler's error).
    Ssh(E),
}

async fn dial<H: client::Handler + Send + 'static>(
    config: Arc<client::Config>,
    host: &str,
    port: u16,
    handler: H,
) -> Result<client::Handle<H>, Dial<H::Error>> {
    let attempt = async {
        let stream = tokio::net::TcpStream::connect((host, port))
            .await
            .map_err(|e| Dial::NoConnection(e.to_string()))?;
        if config.nodelay {
            let _ = stream.set_nodelay(true);
        }
        client::connect_stream(config, stream, handler).await.map_err(Dial::Ssh)
    };
    match tokio::time::timeout(CONNECT_TIMEOUT, attempt).await {
        Err(_) => Err(Dial::NoConnection("timed out".into())),
        Ok(dialled) => dialled,
    }
}

/// The host keys a remote may present, fixed when the remote was added: one
/// key for a host confirmed on first use, or a host's published set (see
/// [`super::hosts`]). Any one of them is accepted; nothing else is.
#[derive(Debug, Clone)]
pub struct HostKeyPin(Vec<PublicKey>);

impl HostKeyPin {
    /// Parses a public key line such as `ssh-ed25519 AAAA...`.
    pub fn parse(line: &str) -> Result<Self, PawError> {
        Self::parse_all(&[line])
    }

    /// Parses several public key lines; the host may present any of them.
    pub fn parse_all(lines: &[&str]) -> Result<Self, PawError> {
        if lines.is_empty() {
            return Err(PawError::Git("no host key to pin".into()));
        }
        lines
            .iter()
            .map(|line| PublicKey::from_openssh(line.trim()))
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
            .map_err(|e| PawError::Git(format!("not a host key: {e}")))
    }

    /// The `SHA256:...` form of every pinned key, preferred first.
    pub fn fingerprints(&self) -> Vec<String> {
        self.0.iter().map(fingerprint_of).collect()
    }

    /// Each pinned key as Settings shows it, preferred first.
    pub fn described(&self) -> Vec<ServerKey> {
        self.0.iter().map(ServerKey::of).collect()
    }

    /// The host key algorithms to offer: the pinned keys' types, preferred
    /// first, and nothing else, so a host with several keys cannot present
    /// one that is not pinned. RSA is left out because this build of russh
    /// has no RSA (see Cargo.toml); a pin of RSA keys only has none.
    pub fn algorithms(&self) -> Vec<Algorithm> {
        let mut offered: Vec<Algorithm> = Vec::new();
        for key in &self.0 {
            let algorithm = key.algorithm();
            if !algorithm.clone().is_rsa() && !offered.contains(&algorithm) {
                offered.push(algorithm);
            }
        }
        offered
    }

    fn accepts(&self, presented: &PublicKey) -> bool {
        self.0.iter().any(|pinned| pinned.key_data() == presented.key_data())
    }
}

/// One host key the store's server must present.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ServerKey {
    /// "Ed25519", "ECDSA P-256", "RSA".
    pub key_type: String,
    /// `SHA256:...`, as `ssh-keygen -lf` and the host's documentation show it.
    pub fingerprint: String,
}

impl ServerKey {
    fn of(key: &PublicKey) -> Self {
        Self { key_type: key_type_words(key.algorithm().as_ref()), fingerprint: fingerprint_of(key) }
    }
}

/// An SSH algorithm name in the words a server's documentation uses.
fn key_type_words(algorithm: &str) -> String {
    match algorithm {
        "ssh-ed25519" => "Ed25519".into(),
        "ecdsa-sha2-nistp256" => "ECDSA P-256".into(),
        "ecdsa-sha2-nistp384" => "ECDSA P-384".into(),
        "ecdsa-sha2-nistp521" => "ECDSA P-521".into(),
        "ssh-rsa" | "rsa-sha2-256" | "rsa-sha2-512" => "RSA".into(),
        other => other.to_owned(),
    }
}

fn fingerprint_of(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// A host key seen on a first connection, before anyone has confirmed it.
#[derive(Debug, Clone)]
pub struct PresentedHostKey(PublicKey);

impl PresentedHostKey {
    /// The `SHA256:...` form to show for confirmation.
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.0)
    }

    pub fn described(&self) -> ServerKey {
        ServerKey::of(&self.0)
    }

    /// The algorithm name, e.g. `ssh-ed25519`.
    pub fn algorithm(&self) -> String {
        self.0.algorithm().to_string()
    }

    /// The key as a public key line, for the known-hosts file.
    pub fn line(&self) -> Result<String, PawError> {
        self.0
            .to_openssh()
            .map_err(|e| PawError::Git(format!("cannot write the host key: {e}")))
    }
}

/// Where the password store lives.
#[derive(Debug, Clone)]
pub struct Remote {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// The repository path as the host expects it after `git-upload-pack`,
    /// e.g. `owner/repo.git` for GitHub.
    pub path: String,
    pub host_key: HostKeyPin,
}

impl Remote {
    /// The address in the form other git tools understand.
    pub fn url(&self) -> String {
        let separator = if self.path.starts_with('/') { "" } else { "/" };
        format!("ssh://{}@{}:{}{separator}{}", self.user, self.host, self.port, self.path)
    }
}

/// An open, authenticated SSH connection to a [`Remote`].
pub struct Connection {
    shared: Arc<Shared>,
}

struct Shared {
    runtime: tokio::runtime::Runtime,
    session: client::Handle<PinnedHost>,
    host: String,
    path: String,
    /// What the remote printed on stderr and how the connection failed, kept
    /// because libgit2 reports either only as "early EOF".
    diagnostics: Mutex<Diagnostics>,
    cap: DownloadCap,
}

#[derive(Default)]
struct Diagnostics {
    /// The service runs `git-upload-pack`: a fetch or a clone.
    fetching: bool,
    stderr: Vec<u8>,
    network: Option<String>,
}

/// Accepts the pinned host key and nothing else.
struct PinnedHost {
    pin: HostKeyPin,
    /// Fingerprint of a key that was presented and refused.
    refused: Arc<Mutex<Option<String>>>,
}

impl client::Handler for PinnedHost {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let presented = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key,
            // Host certificates are not pinned keys.
            PublicKeyOrCertificate::Certificate(_) => {
                *self.refused.lock().unwrap() = Some("a host certificate".into());
                return Ok(false);
            }
        };
        if self.pin.accepts(presented) {
            return Ok(true);
        }
        *self.refused.lock().unwrap() = Some(fingerprint_of(presented));
        Ok(false)
    }
}

/// Accepts no key, and remembers the one the host presented: how a host
/// that is not pinned yet is asked for its key, with nothing sent to it.
struct RecordHost {
    presented: Arc<Mutex<Option<PublicKey>>>,
}

impl client::Handler for RecordHost {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        if let PublicKeyOrCertificate::PublicKey { key, .. } = key {
            *self.presented.lock().unwrap() = Some(key.clone());
        }
        Ok(false)
    }
}

/// Stands in for the card when the host is only asked whether it would
/// accept a key. It is called only if the host said yes, and it never signs.
struct ProbeOnly {
    asked: bool,
}

impl russh::Signer for ProbeOnly {
    type Error = LoginError;

    async fn auth_sign(
        &mut self,
        _key: &AgentIdentity,
        _hash_alg: Option<HashAlg>,
        _signed: Vec<u8>,
    ) -> Result<Vec<u8>, LoginError> {
        self.asked = true;
        Err(LoginError::OnlyAsking)
    }
}

/// Why a login did not finish, in the signer's error type.
enum LoginError {
    Key(SignFailure),
    /// The session closed during the login.
    Closed,
    /// [`ProbeOnly`] does not sign.
    OnlyAsking,
}

/// russh needs to report a closed session through the signer's error type.
impl From<russh::SendError> for LoginError {
    fn from(_: russh::SendError) -> Self {
        Self::Closed
    }
}

impl LoginError {
    fn on(self, remote: &Remote) -> ConnectError {
        match self {
            Self::Key(SignFailure::Key(error)) => ConnectError::Key(error),
            Self::Key(SignFailure::Cancelled) => ConnectError::Cancelled,
            Self::Closed | Self::OnlyAsking => {
                ConnectError::Fault(unreachable(&remote.host, "the connection closed during login"))
            }
        }
    }
}

fn runtime(host: &str) -> Result<tokio::runtime::Runtime, Fault> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| unreachable(host, e))
}

/// Connects and checks the host key against `remote`'s pin. Nothing has
/// been sent that identifies the user when this returns.
async fn handshake(remote: &Remote) -> Result<client::Handle<PinnedHost>, Fault> {
    let pin = remote.host_key.clone();
    let mut config = client::Config {
        inactivity_timeout: Some(INACTIVITY_TIMEOUT),
        ..Default::default()
    };
    let algorithms = pin.algorithms();
    if algorithms.is_empty() {
        return Err(PawError::Git("none of the pinned host keys is of a type paw supports".into()).into());
    }
    config.preferred.key = algorithms.into();

    let refused = Arc::new(Mutex::new(None));
    let handler = PinnedHost { pin, refused: refused.clone() };
    match dial(Arc::new(config), &remote.host, remote.port, handler).await {
        Err(Dial::NoConnection(why)) => Err(no_connection(&remote.host, remote.port, &why)),
        Err(Dial::Ssh(e)) => Err(match refused.lock().unwrap().take() {
            Some(presented) => RemoteFailure::HostKeyChanged {
                host: remote.host.clone(),
                expected: remote.host_key.fingerprints(),
                presented,
                bundled: super::hosts::is_bundled(&remote.host, remote.port),
            }
            .into(),
            None => unreachable(&remote.host, e),
        }),
        Ok(session) => Ok(session),
    }
}

/// Connects to `host` and returns the host key it presents, without
/// accepting it or sending anything else. For a host that is not pinned
/// yet: the key is shown to the person, and pinned only once confirmed.
pub fn scan_host_key(host: &str, port: u16) -> Result<PresentedHostKey, Fault> {
    let runtime = runtime(host)?;
    let presented = Arc::new(Mutex::new(None));
    let handler = RecordHost { presented: presented.clone() };
    let config = client::Config {
        inactivity_timeout: Some(INACTIVITY_TIMEOUT),
        ..Default::default()
    };
    let outcome = runtime.block_on(dial(Arc::new(config), host, port, handler));
    let error = match outcome {
        Err(Dial::NoConnection(why)) => no_connection(host, port, &why),
        // The handler refuses every key, so the handshake cannot succeed.
        Ok(_) => unreachable(host, "the host did not ask to be checked"),
        Err(Dial::Ssh(e)) => unreachable(host, e),
    };
    match presented.lock().unwrap().take() {
        Some(key) => Ok(PresentedHostKey(key)),
        None => Err(error),
    }
}

/// Lets russh ask an [`SshSigner`] for the login signature.
struct LoginSigner<'a>(&'a dyn SshSigner);

/// Why [`Connection::open`] did not log in.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// The key failed to sign the login (no touch, unplugged, PIN gone).
    #[error(transparent)]
    Key(KeyError),
    /// The key task was cancelled during the login.
    #[error("cancelled")]
    Cancelled,
    /// The host, the network or the host key.
    #[error(transparent)]
    Fault(Fault),
}

impl From<Fault> for ConnectError {
    fn from(fault: Fault) -> Self {
        Self::Fault(fault)
    }
}

impl russh::Signer for LoginSigner<'_> {
    type Error = LoginError;

    async fn auth_sign(
        &mut self,
        _key: &AgentIdentity,
        _hash_alg: Option<HashAlg>,
        mut signed: Vec<u8>,
    ) -> Result<Vec<u8>, LoginError> {
        // This blocks, possibly on a touch. It runs on the thread that called
        // `Connection::open`, not on the runtime's worker, so the session
        // keeps answering the host meanwhile.
        let signature = self.0.sign(&signed).map_err(LoginError::Key)?;

        // russh wants the signed data back with the SSH signature appended:
        // string(string(algorithm) string(signature)).
        let algorithm = self.0.public_key().algorithm().as_bytes();
        let fields = [algorithm, &signature[..]];
        let length: usize = fields.iter().map(|f| 4 + f.len()).sum();
        signed.extend((length as u32).to_be_bytes());
        for field in fields {
            signed.extend((field.len() as u32).to_be_bytes());
            signed.extend(field);
        }
        Ok(signed)
    }
}

impl Connection {
    /// Connects, checks the host key against the pin, and logs in with one
    /// signature from `signer`.
    ///
    /// The host is asked whether it accepts the public key before the signer
    /// is used, so a key the host does not know costs no PIN try and no
    /// touch: that case is [`RemoteFailure::KeyNotAccepted`].
    pub fn open(remote: &Remote, signer: &dyn SshSigner) -> Result<Self, ConnectError> {
        let runtime = runtime(&remote.host)?;
        let session = runtime.block_on(async {
            let mut session = handshake(remote).await?;
            let key = login_key(signer.public_key())?;
            let login = session
                .authenticate_publickey_with(&remote.user, key, None, &mut LoginSigner(signer))
                .await
                .map_err(|e| e.on(remote))?;
            match login {
                AuthResult::Success => Ok(session),
                AuthResult::Failure { .. } => Err(ConnectError::Fault(not_accepted(remote, signer.public_key()))),
            }
        })?;

        Ok(Self {
            shared: Arc::new(Shared {
                runtime,
                session,
                host: remote.host.clone(),
                path: remote.path.clone(),
                diagnostics: Mutex::default(),
                cap: DownloadCap::new(DOWNLOAD_LIMIT),
            }),
        })
    }

    /// Asks the host whether it would accept `key` for `remote.user`,
    /// without a signature: no PIN, no touch. Checks the host key first,
    /// exactly as [`Self::open`] does.
    pub fn probe(remote: &Remote, key: &SshPublicKey) -> Result<(), Fault> {
        let runtime = runtime(&remote.host)?;
        let accepted = runtime.block_on(async {
            let mut session = handshake(remote).await?;
            let public = login_key(key)?;
            let mut probe = ProbeOnly { asked: false };
            let answer = session
                .authenticate_publickey_with(&remote.user, public, None, &mut probe)
                .await;
            let _ = session.disconnect(Disconnect::ByApplication, "", "en").await;
            match answer {
                Ok(AuthResult::Success) => Ok(true),
                Ok(AuthResult::Failure { .. }) => Ok(probe.asked),
                Err(_) if probe.asked => Ok(true),
                Err(_) => Err(unreachable(&remote.host, "the connection closed during login")),
            }
        })?;
        if accepted { Ok(()) } else { Err(not_accepted(remote, key)) }
    }

    /// Sets the most one fetch, clone or push on this connection may
    /// receive, in bytes, in place of [`DOWNLOAD_LIMIT`]. One that would
    /// receive more stops saying so.
    #[cfg(feature = "sim")]
    pub fn set_download_limit(&self, bytes: u64) {
        self.shared.cap.set_limit(bytes);
    }

    /// Runs `operation` against a libgit2 remote whose transport is this
    /// connection.
    pub(super) fn with_remote<T>(
        &self,
        repo: &git2::Repository,
        operation: impl FnOnce(&mut git2::Remote<'_>) -> Result<T, git2::Error>,
    ) -> Result<T, Fault> {
        *self.shared.diagnostics.lock().unwrap() = Diagnostics::default();
        git_transport::with_remote(repo, self.shared.clone(), operation, |e| self.shared.explain(e))
    }
}

fn login_key(key: &SshPublicKey) -> Result<PublicKey, Fault> {
    PublicKey::from_bytes(key.blob()).map_err(|e| RemoteFailure::LoginKeyUnusable { detail: e.to_string() }.into())
}

fn not_accepted(remote: &Remote, key: &SshPublicKey) -> Fault {
    RemoteFailure::KeyNotAccepted { host: remote.host.clone(), public_key: key.authorized_key() }.into()
}

/// Whether what a host printed when a git command failed says the
/// repository is not there, or not there for this key. Hosts word it
/// differently:
/// - GitHub: "ERROR: Repository not found."
/// - GitLab: "ERROR: The project you were looking for could not be found or
///   you don't have permission to view it."
/// - Gitea, Forgejo (Codeberg): "... Repository does not exist"
/// - Bitbucket: "repository does not exist", "Forbidden"
/// - git-daemon and plain `git-upload-pack`: "... does not appear to be a
///   git repository"
fn says_repository_missing(said: &str) -> bool {
    let said = said.to_ascii_lowercase();
    if said.contains("command not found") {
        return false;
    }
    [
        "repository not found",
        "could not be found",
        "does not exist",
        "not appear to be a git repository",
        "you don't have permission",
        "access denied",
        "forbidden",
    ]
    .iter()
    .any(|phrase| said.contains(phrase))
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Best effort: the host learns the session is over instead of waiting
        // for the TCP connection to drop.
        let _ = self.shared.runtime.block_on(self.shared.session.disconnect(
            Disconnect::ByApplication,
            "",
            "en",
        ));
    }
}

impl Shared {
    fn explain(&self, error: git2::Error) -> Fault {
        let mut diagnostics = self.diagnostics.lock().unwrap();
        if let Some(network) = diagnostics.network.take() {
            return unreachable(&self.host, network);
        }
        let stderr = String::from_utf8_lossy(&diagnostics.stderr);
        let detail = match stderr.trim() {
            "" => PawError::Git(error.message().to_owned()),
            said => PawError::Git(format!("{} (the host said: {said})", error.message())),
        };
        match said(&diagnostics.stderr) {
            Some(said) if diagnostics.fetching && says_repository_missing(&said) => RemoteFailure::RepositoryMissing {
                host: self.host.clone(),
                repository: self.path.trim_start_matches('/').to_owned(),
                said: Some(said),
                detail: detail.to_string(),
            }
            .into(),
            _ => detail.into(),
        }
    }

    fn network_failure(&self, error: russh::Error) -> io::Error {
        let message = error.to_string();
        self.diagnostics.lock().unwrap().network = Some(message.clone());
        io::Error::other(message)
    }
}

impl Carrier for Shared {
    fn stateless(&self) -> bool {
        // One channel per service, kept open for the whole exchange.
        false
    }

    fn open(self: Arc<Self>, service: Service) -> Result<Box<dyn SmartSubtransportStream>, git2::Error> {
        let program = match service {
            Service::UploadPackLs => "git-upload-pack",
            Service::ReceivePackLs => "git-receive-pack",
            // libgit2 reuses the stream of the matching "Ls" action.
            Service::UploadPack | Service::ReceivePack => {
                return Err(git2::Error::from_str("unexpected second stream for one service"));
            }
        };
        let command = format!("{program} {}", shell_quote(&self.path));
        self.diagnostics.lock().unwrap().fetching = program == "git-upload-pack";

        let channel = self
            .runtime
            .block_on(async {
                let channel = self.session.channel_open_session().await?;
                channel.exec(true, command).await?;
                Ok(channel)
            })
            .map_err(|e| git2::Error::from_str(&self.network_failure(e).to_string()))?;

        Ok(Box::new(ServiceStream { shared: self, channel, unread: Vec::new() }))
    }

    fn cap(&self) -> &DownloadCap {
        &self.cap
    }
}

/// What the host printed on stderr, e.g. GitHub's "ERROR: Repository not
/// found.". It is the host's text, not paw's: show it only as a quote.
fn said(stderr: &[u8]) -> Option<String> {
    let said: String = String::from_utf8_lossy(stderr)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let said = said.trim();
    (!said.is_empty()).then(|| said.to_owned())
}

/// Quotes a repository path for the remote shell, as git itself does.
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', r"'\''"))
}

/// The stdin and stdout of one git service on the host, as blocking I/O.
struct ServiceStream {
    shared: Arc<Shared>,
    channel: Channel<client::Msg>,
    /// Received bytes libgit2 has not asked for yet.
    unread: Vec<u8>,
}

impl Read for ServiceStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.unread.is_empty() {
            match self.shared.runtime.block_on(self.channel.wait()) {
                Some(ChannelMsg::Data { data }) => {
                    self.shared.cap.count(data.len())?;
                    self.unread = data.to_vec();
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let stderr = &mut self.shared.diagnostics.lock().unwrap().stderr;
                    let room = STDERR_LIMIT.saturating_sub(stderr.len());
                    stderr.extend(data.iter().take(room));
                }
                Some(ChannelMsg::Failure) => {
                    return Err(io::Error::other("the host refused to run git"));
                }
                Some(ChannelMsg::Eof | ChannelMsg::Close) | None => return Ok(0),
                Some(_) => {}
            }
        }
        let n = buf.len().min(self.unread.len());
        buf[..n].copy_from_slice(&self.unread[..n]);
        self.unread.drain(..n);
        Ok(n)
    }
}

impl Write for ServiceStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.shared
            .runtime
            .block_on(self.channel.data_bytes(buf.to_vec()))
            .map_err(|e| self.shared.network_failure(e))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for ServiceStream {
    fn drop(&mut self) {
        let _ = self.shared.runtime.block_on(self.channel.close());
    }
}

#[cfg(test)]
mod tests {
    use super::{no_connection, says_repository_missing};
    use crate::fault::Fault;
    use crate::remote::{PortFallback, RemoteFailure};

    /// What a failed connection to `host:port` offers instead.
    fn offered(host: &str, port: u16) -> Option<PortFallback> {
        match no_connection(host, port, "connection refused") {
            Fault::Remote { failure: RemoteFailure::NoConnection { fallback, .. } } => fallback,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn only_github_on_port_22_offers_port_443() {
        let to_443 = PortFallback { from_host: "github.com".into(), from_port: 22, host: "ssh.github.com".into(), port: 443 };
        assert_eq!(offered("github.com", 22), Some(to_443));
        for (host, port) in [("github.com", 2222), ("ssh.github.com", 443), ("gitlab.com", 22), ("127.0.0.1", 22)] {
            assert_eq!(offered(host, port), None, "{host}:{port}");
        }
    }

    #[test]
    fn hosts_wordings_of_a_missing_repository_are_recognised() {
        for said in [
            "ERROR: Repository not found.",
            "ERROR: The project you were looking for could not be found or you don't have permission to view it.",
            "Gitea: Repository does not exist",
            "fatal: '/srv/x.git' does not appear to be a git repository",
            "Forbidden",
        ] {
            assert!(says_repository_missing(said), "{said}");
        }
        for said in ["", "sh: git-upload-pack: command not found", "fatal: protocol error: bad line length"] {
            assert!(!says_repository_missing(said), "{said}");
        }
    }
}

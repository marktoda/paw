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

use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use git2::transport::{Service, SmartSubtransport, SmartSubtransportStream, Transport};
use russh::client::{self, AuthResult};
use russh::keys::agent::AgentIdentity;
use russh::keys::{HashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg, Disconnect};

use crate::PawError;
use crate::ssh::SshSigner;

/// How long to wait for the TCP connection and SSH handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// A connection that carries nothing for this long is treated as dead, so a
/// phone that lost its network does not hang in the middle of a fetch.
const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(60);
/// The URL libgit2 is given. It only selects our transport; the connection
/// itself comes from [`ACTIVE`].
const TRANSPORT_URL: &str = "paw-ssh://connection";
/// How much of the remote's stderr is kept for error messages.
const STDERR_LIMIT: usize = 2048;

/// The host key a remote must present, fixed when the remote was added.
#[derive(Debug, Clone)]
pub struct HostKeyPin(PublicKey);

impl HostKeyPin {
    /// Parses a public key line such as `ssh-ed25519 AAAA...`.
    pub fn parse(line: &str) -> Result<Self, PawError> {
        PublicKey::from_openssh(line)
            .map(Self)
            .map_err(|e| PawError::Git(format!("not a host key: {e}")))
    }

    /// The `SHA256:...` form shown to the user.
    pub fn fingerprint(&self) -> String {
        self.0.fingerprint(HashAlg::Sha256).to_string()
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
    path: String,
    /// What the remote printed on stderr and how the connection failed, kept
    /// because libgit2 reports either only as "early EOF".
    diagnostics: Mutex<Diagnostics>,
}

#[derive(Default)]
struct Diagnostics {
    stderr: Vec<u8>,
    network: Option<String>,
}

/// Accepts the pinned host key and nothing else.
struct PinnedHost {
    pin: PublicKey,
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
        if presented.key_data() == self.pin.key_data() {
            return Ok(true);
        }
        *self.refused.lock().unwrap() = Some(presented.fingerprint(HashAlg::Sha256).to_string());
        Ok(false)
    }
}

/// Lets russh ask an [`SshSigner`] for the login signature.
struct LoginSigner<'a>(&'a dyn SshSigner);

/// russh needs to report a closed session through the signer's error type.
impl From<russh::SendError> for PawError {
    fn from(_: russh::SendError) -> Self {
        PawError::Network("the connection closed during login".into())
    }
}

impl russh::Signer for LoginSigner<'_> {
    type Error = PawError;

    async fn auth_sign(
        &mut self,
        _key: &AgentIdentity,
        _hash_alg: Option<HashAlg>,
        mut signed: Vec<u8>,
    ) -> Result<Vec<u8>, PawError> {
        // This blocks, possibly on a touch. It runs on the thread that called
        // `Connection::open`, not on the runtime's worker, so the session
        // keeps answering the host meanwhile.
        let signature = self.0.sign(&signed)?;

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
    /// touch: that case is [`PawError::KeyRejected`].
    pub fn open(remote: &Remote, signer: &dyn SshSigner) -> Result<Self, PawError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| PawError::Network(e.to_string()))?;

        let pin = remote.host_key.0.clone();
        let mut config = client::Config {
            inactivity_timeout: Some(INACTIVITY_TIMEOUT),
            ..Default::default()
        };
        // Ask only for the pinned key's type. A host with several host keys
        // would otherwise be free to present one that cannot match the pin.
        config.preferred.key = vec![pin.algorithm()].into();

        let refused = Arc::new(Mutex::new(None));
        let handler = PinnedHost { pin, refused: refused.clone() };
        let address = (remote.host.as_str(), remote.port);

        let session = runtime.block_on(async {
            let connecting = client::connect(Arc::new(config), address, handler);
            let mut session = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
                Err(_) => return Err(PawError::Network("timed out".into())),
                Ok(Err(e)) => {
                    return Err(match refused.lock().unwrap().take() {
                        Some(presented) => PawError::HostKeyMismatch { presented },
                        None => PawError::Network(e.to_string()),
                    });
                }
                Ok(Ok(session)) => session,
            };

            let key = PublicKey::from_bytes(signer.public_key().blob())
                .map_err(|e| PawError::NoSshKey(e.to_string()))?;
            let login = session
                .authenticate_publickey_with(&remote.user, key, None, &mut LoginSigner(signer))
                .await?;
            match login {
                AuthResult::Success => Ok(session),
                AuthResult::Failure { .. } => Err(PawError::KeyRejected),
            }
        })?;

        Ok(Self {
            shared: Arc::new(Shared {
                runtime,
                session,
                path: remote.path.clone(),
                diagnostics: Mutex::default(),
            }),
        })
    }

    /// Runs `operation` against a libgit2 remote whose transport is this
    /// connection.
    pub(super) fn with_remote<T>(
        &self,
        repo: &git2::Repository,
        operation: impl FnOnce(&mut git2::Remote<'_>) -> Result<T, git2::Error>,
    ) -> Result<T, PawError> {
        register_transport();
        *self.shared.diagnostics.lock().unwrap() = Diagnostics::default();

        ACTIVE.with(|active| *active.borrow_mut() = Some(self.shared.clone()));
        let result = repo
            .remote_anonymous(TRANSPORT_URL)
            .and_then(|mut remote| operation(&mut remote));
        ACTIVE.with(|active| *active.borrow_mut() = None);

        result.map_err(|e| self.shared.explain(e))
    }
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
    /// Turns a libgit2 failure into the most specific error we know.
    fn explain(&self, error: git2::Error) -> PawError {
        if error.code() == git2::ErrorCode::NotFastForward {
            return PawError::NotFastForward;
        }
        let mut diagnostics = self.diagnostics.lock().unwrap();
        if let Some(network) = diagnostics.network.take() {
            return PawError::Network(network);
        }
        let stderr = String::from_utf8_lossy(&diagnostics.stderr);
        match stderr.trim() {
            "" => PawError::Git(error.message().to_owned()),
            said => PawError::Git(format!("{} (the host said: {said})", error.message())),
        }
    }

    fn network_failure(&self, error: russh::Error) -> io::Error {
        let message = error.to_string();
        self.diagnostics.lock().unwrap().network = Some(message.clone());
        io::Error::other(message)
    }
}

thread_local! {
    /// The connection the libgit2 call running on this thread should use.
    /// libgit2 picks a transport by URL scheme through a process-wide table,
    /// which has no room for a per-call value; the call is synchronous, so
    /// the calling thread carries it instead.
    static ACTIVE: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) };
}

fn register_transport() {
    static REGISTERED: Once = Once::new();
    REGISTERED.call_once(|| {
        let factory = |remote: &git2::Remote<'_>| {
            let shared = ACTIVE
                .with(|active| active.borrow().clone())
                .ok_or_else(|| git2::Error::from_str("no SSH connection is open for this call"))?;
            // `false`: the protocol is stateful, one channel per service.
            Transport::smart(remote, false, Subtransport(shared))
        };
        // SAFETY: libgit2 requires that no other thread creates a transport
        // while the table changes. `Once` serialises registration, and every
        // transport in this process is created by `with_remote`, which calls
        // this first.
        unsafe { git2::transport::register("paw-ssh", factory) }
            .expect("registering the SSH transport with libgit2");
    });
}

struct Subtransport(Arc<Shared>);

impl SmartSubtransport for Subtransport {
    fn action(
        &self,
        _url: &str,
        service: Service,
    ) -> Result<Box<dyn SmartSubtransportStream>, git2::Error> {
        let program = match service {
            Service::UploadPackLs => "git-upload-pack",
            Service::ReceivePackLs => "git-receive-pack",
            // libgit2 reuses the stream of the matching "Ls" action.
            Service::UploadPack | Service::ReceivePack => {
                return Err(git2::Error::from_str("unexpected second stream for one service"));
            }
        };
        let command = format!("{program} {}", shell_quote(&self.0.path));

        let shared = self.0.clone();
        let channel = shared
            .runtime
            .block_on(async {
                let channel = shared.session.channel_open_session().await?;
                channel.exec(true, command).await?;
                Ok(channel)
            })
            .map_err(|e| git2::Error::from_str(&shared.network_failure(e).to_string()))?;

        Ok(Box::new(ServiceStream { shared, channel, unread: Vec::new() }))
    }

    fn close(&self) -> Result<(), git2::Error> {
        // The channel closes when its stream is dropped. The connection stays
        // open for the next service.
        Ok(())
    }
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
                Some(ChannelMsg::Data { data }) => self.unread = data.to_vec(),
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

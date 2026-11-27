//! A simulated git host: an SSH server on a loopback port that serves one
//! bare repository, for tests and for running setup end to end in the iOS
//! simulator with the [`SimulatedKey`].
//!
//! Everything runs in this process. The SSH side is russh's server; the git
//! side is `git-upload-pack` and `git-receive-pack` written here on top of
//! libgit2 (protocol version 0, the subset libgit2's client speaks): no
//! `git` program is started, so it works where none exists or none may be
//! started, as in an iOS app.
//!
//! What it supports is what paw needs and no more:
//! - upload-pack: the ref advertisement (with `symref=HEAD:...` so the
//!   client finds the default branch), wants and haves without `multi_ack`
//!   or side-band, and a pack built by libgit2's pack builder;
//! - receive-pack: fast-forward updates of branches under `refs/heads/`
//!   with `report-status`, the pack indexed by libgit2. No deletes, no
//!   tags, no shallow clones.
//!
//! Its host keys are fixed values in this file and protect nothing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use git2::{Oid, Repository};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{HashAlg, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, ChannelMsg, ChannelReadHalf, Disconnect};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::SimulatedKey;
use crate::locked;
use crate::PawError;
use crate::ssh::SshPublicKey;
use crate::sync::{HostKeyPin, Remote};

pub const REPO: &str = "store.git";
const BRANCH: &str = "refs/heads/main";
const USER: &str = "git";
pub(crate) const AGENT: &str = "agent=paw-sim/1";
/// The most a push may send. The simulated store is a few kilobytes.
const MAX_PUSH: usize = 64 << 20;
const ZERO_HEX: &str = "0000000000000000000000000000000000000000";

/// An ECDSA P-256 host key, made with ssh-keygen for the simulated host. It
/// protects nothing.
const ECDSA_HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAaAAAABNlY2RzYS
1zaGEyLW5pc3RwMjU2AAAACG5pc3RwMjU2AAAAQQSDeBXw2cY7/bLcIZ+3/uD3+6arc22x
kB+w7P9NzlaYwa0zuD/P3nJG8zxGMSKapeTExcKjUTPp2L4xoi/2K9vmAAAAqE1dnpZNXZ
6WAAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBIN4FfDZxjv9stwh
n7f+4Pf7pqtzbbGQH7Ds/03OVpjBrTO4P8/eckbzPEYxIpql5MTFwqNRM+nYvjGiL/Yr2+
YAAAAgKugwxnhe6unKXb4Ima+fATA3weyrgARUpgolnBVeVmAAAAANcGF3IHRlc3QgaG9z
dAECAw==
-----END OPENSSH PRIVATE KEY-----
";

fn ed25519_host(seed: u8) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
}

fn line_of(key: &PrivateKey) -> String {
    key.public_key().to_openssh().unwrap_or_default()
}

pub fn ed25519_host_key() -> String {
    line_of(&ed25519_host(1))
}

/// The public key line of the key the host presents once told its key
/// changed.
pub fn changed_host_key() -> String {
    line_of(&ed25519_host(2))
}

/// The public key line of the ECDSA key a host started with
/// [`SimRemoteOptions::ecdsa_host_key`] also holds.
pub fn ecdsa_host_key() -> String {
    PrivateKey::from_openssh(ECDSA_HOST_KEY).map(|key| line_of(&key)).unwrap_or_default()
}

#[derive(Debug, Clone)]
pub enum SimRemoteContents {
    Empty,
    /// One commit adding these files (path, contents).
    Files(Vec<(String, Vec<u8>)>),
    /// One commit adding the fixture store ([`super::fixture`]).
    FixtureStore,
}

#[derive(Debug, Clone)]
pub struct SimRemoteOptions {
    /// Public key lines (`ssh-ed25519 AAAA...`) that may log in.
    pub authorised: Vec<String>,
    pub contents: SimRemoteContents,
    /// Hold an ECDSA host key besides the ed25519 one.
    pub ecdsa_host_key: bool,
}

type BeforePush = Box<dyn FnOnce() + Send>;

struct Shared {
    repo: PathBuf,
    authorised: Mutex<Vec<PublicKey>>,
    refuse_key: AtomicBool,
    changed_host_key: AtomicBool,
    drop_mid_pack: AtomicBool,
    refuse_pushes: AtomicBool,
    repository_not_found: AtomicBool,
    logins: AtomicU32,
    services: Mutex<Vec<Vec<String>>>,
    before_push: Mutex<Option<BeforePush>>,
}


#[derive(uniffi::Object)]
pub struct SimRemote {
    runtime: Option<tokio::runtime::Runtime>,
    root: PathBuf,
    /// The folder was given, not made: it stays when the host stops.
    keep_root: bool,
    port: u16,
    shared: Arc<Shared>,
}

fn failed(what: &str, error: impl std::fmt::Display) -> PawError {
    PawError::Git(format!("simulated host: could not {what}: {error}"))
}

fn git_failed(what: &str) -> impl Fn(git2::Error) -> PawError + '_ {
    move |e| failed(what, e.message())
}

fn fresh_dir() -> Result<PathBuf, PawError> {
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let name: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("paw-sim-remote-{name}"));
    std::fs::create_dir_all(&dir).map_err(|e| failed("make its folder", e))?;
    Ok(dir)
}

fn files_under(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, PawError> {
    let mut found = Vec::new();
    let mut folders = vec![(dir.to_path_buf(), String::new())];
    while let Some((folder, prefix)) = folders.pop() {
        let entries = std::fs::read_dir(&folder).map_err(|e| failed("read the fixture", e))?;
        for entry in entries {
            let entry = entry.map_err(|e| failed("read the fixture", e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = format!("{prefix}{name}");
            let kind = entry.file_type().map_err(|e| failed("read the fixture", e))?;
            if kind.is_dir() {
                folders.push((entry.path(), format!("{path}/")));
            } else if kind.is_file() {
                found.push((path, std::fs::read(entry.path()).map_err(|e| failed("read the fixture", e))?));
            }
        }
    }
    found.sort();
    Ok(found)
}

fn commit_files(repo: &Path, files: &[(String, Vec<u8>)], message: &str) -> Result<Oid, PawError> {
    let repo = Repository::open_bare(repo).map_err(git_failed("open the repository"))?;
    let parent = repo.refname_to_id(BRANCH).ok().and_then(|id| repo.find_commit(id).ok());
    let mut index = git2::Index::new().map_err(git_failed("make an index"))?;
    if let Some(parent) = &parent {
        let tree = parent.tree().map_err(git_failed("read the tree"))?;
        index.read_tree(&tree).map_err(git_failed("read the tree"))?;
    }
    for (path, contents) in files {
        let blob = repo.blob(contents).map_err(git_failed("write a file"))?;
        index.add(&crate::sync::history::index_entry(path, blob, git2::FileMode::Blob.into())).map_err(git_failed("add a file"))?;
    }
    let tree = index.write_tree_to(&repo).map_err(git_failed("write the tree"))?;
    let tree = repo.find_tree(tree).map_err(git_failed("write the tree"))?;
    let when = git2::Time::new(i64::from(super::CREATED), 0);
    let signature = git2::Signature::new("Desktop", "desktop@example.com", &when).map_err(git_failed("sign"))?;
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
    repo.commit(Some(BRANCH), &signature, &signature, message, &tree, &parents)
        .map_err(git_failed("commit"))
}

/// A host on the same port that was just stopped may hold it a moment
/// longer, so a busy port is tried again for a few seconds.
async fn bind(port: u16) -> Result<tokio::net::TcpListener, PawError> {
    let mut tries = 0;
    loop {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok(listener),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && port != 0 && tries < 60 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(failed(&format!("listen on port {port}"), e)),
        }
    }
}

fn fixture_options(key: &SimulatedKey) -> SimRemoteOptions {
    SimRemoteOptions {
        authorised: vec![SshPublicKey::ed25519(&key.authentication_public_key()).authorized_key()],
        contents: SimRemoteContents::FixtureStore,
        ecdsa_host_key: false,
    }
}

impl SimRemote {
    pub fn start_with(options: SimRemoteOptions) -> Result<Arc<Self>, PawError> {
        Self::start_with_on(options, 0)
    }

    /// Starts a host as `options` say, listening on `port` of 127.0.0.1
    /// (0: any free port).
    pub fn start_with_on(options: SimRemoteOptions, port: u16) -> Result<Arc<Self>, PawError> {
        let root = fresh_dir()?;
        let started = Self::start_in(&root, false, options, port);
        if started.is_err() {
            let _ = std::fs::remove_dir_all(&root);
        }
        started
    }

    /// Serves the repository kept in `dir` (made with `options.contents`
    /// if it has none yet) on `port`. The folder stays when the host stops.
    pub fn serve_with(dir: &Path, options: SimRemoteOptions, port: u16) -> Result<Arc<Self>, PawError> {
        std::fs::create_dir_all(dir).map_err(|e| failed("make its folder", e))?;
        Self::start_in(dir, true, options, port)
    }

    fn start_in(root: &Path, keep_root: bool, options: SimRemoteOptions, port: u16) -> Result<Arc<Self>, PawError> {
        let repo = root.join(REPO);
        if Repository::open_bare(&repo).is_err() {
            Self::make_repository(root, &repo, &options.contents)?;
        }
        Self::listen(root, keep_root, repo, &options, port)
    }

    fn make_repository(root: &Path, repo: &Path, contents: &SimRemoteContents) -> Result<(), PawError> {
        let mut init = git2::RepositoryInitOptions::new();
        init.bare(true).initial_head("main");
        Repository::init_opts(repo, &init).map_err(git_failed("make the repository"))?;
        match contents {
            SimRemoteContents::Empty => {}
            SimRemoteContents::Files(files) => {
                commit_files(repo, files, "A password store.")?;
            }
            SimRemoteContents::FixtureStore => {
                let fixture = root.join("fixture");
                super::fixture::write_fixture_store(fixture.to_string_lossy().into_owned())?;
                commit_files(repo, &files_under(&fixture)?, "The fixture store.")?;
                std::fs::remove_dir_all(&fixture).map_err(|e| failed("tidy up", e))?;
            }
        }
        Ok(())
    }

    fn listen(
        root: &Path,
        keep_root: bool,
        repo: PathBuf,
        options: &SimRemoteOptions,
        port: u16,
    ) -> Result<Arc<Self>, PawError> {

        let authorised = options
            .authorised
            .iter()
            .map(|line| PublicKey::from_openssh(line.trim()).map_err(|e| failed("read an authorised key", e)))
            .collect::<Result<Vec<_>, _>>()?;
        let shared = Arc::new(Shared {
            repo,
            authorised: Mutex::new(authorised),
            refuse_key: AtomicBool::new(false),
            changed_host_key: AtomicBool::new(false),
            drop_mid_pack: AtomicBool::new(false),
            refuse_pushes: AtomicBool::new(false),
            repository_not_found: AtomicBool::new(false),
            logins: AtomicU32::new(0),
            services: Mutex::new(Vec::new()),
            before_push: Mutex::new(None),
        });

        let mut keys = vec![ed25519_host(1)];
        if options.ecdsa_host_key {
            keys.push(PrivateKey::from_openssh(ECDSA_HOST_KEY).map_err(|e| failed("read its host key", e))?);
        }
        let config = |keys: Vec<PrivateKey>| {
            Arc::new(server::Config {
                keys,
                // Refusals are delayed by default to slow down guessing.
                auth_rejection_time: Duration::ZERO,
                auth_rejection_time_initial: Some(Duration::ZERO),
                ..Default::default()
            })
        };
        let usual = config(keys);
        let changed = config(vec![ed25519_host(2)]);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| failed("start", e))?;
        let listener = runtime.block_on(bind(port))?;
        let port = listener.local_addr().map_err(|e| failed("listen", e))?.port();
        let accepting = shared.clone();
        runtime.spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let config = if accepting.changed_host_key.load(Ordering::SeqCst) { &changed } else { &usual };
                let handler = Connection { shared: accepting.clone(), login: None, channels: Vec::new() };
                let config = config.clone();
                tokio::spawn(async move {
                    if let Ok(session) = server::run_stream(config, socket, handler).await {
                        let _ = session.await;
                    }
                });
            }
        });

        Ok(Arc::new(Self { runtime: Some(runtime), root: root.to_path_buf(), keep_root, port, shared }))
    }

    /// The host as paw's sync code addresses it, pinned to its usual key.
    pub fn remote(&self) -> Result<Remote, PawError> {
        Ok(Remote {
            host: "127.0.0.1".into(),
            port: self.port,
            user: USER.into(),
            path: REPO.into(),
            host_key: HostKeyPin::parse(&ed25519_host_key())?,
        })
    }

    pub fn bare(&self) -> PathBuf {
        self.shared.repo.clone()
    }

    /// The git services each logged-in connection ran, in login order.
    pub fn services(&self) -> Vec<Vec<String>> {
        locked(&self.shared.services).clone()
    }

    /// Runs `change` just before the next `git-receive-pack` lists its
    /// branches: another machine pushing while paw's push is under way.
    pub fn before_next_push(&self, change: impl FnOnce() + Send + 'static) {
        *locked(&self.shared.before_push) = Some(Box::new(change));
    }
}

#[uniffi::export]
impl SimRemote {
    /// Starts a host on a loopback port serving a fresh copy of the fixture
    /// store, accepting `key`'s SSH key.
    #[uniffi::constructor]
    pub fn start(key: Arc<SimulatedKey>) -> Result<Arc<Self>, PawError> {
        Self::start_with(fixture_options(&key))
    }

    /// Serves the repository kept in the folder `dir` on `port` (0: any
    /// free port), accepting `key`'s SSH key. The first time, the folder
    /// gets a fresh copy of the fixture store; after that, whatever the
    /// repository holds (pushes included) is served again. The folder stays
    /// when the host stops, and the host keys are always the same, so an
    /// app launched again can serve the same store at the same address and
    /// its pinned host key still holds.
    #[uniffi::constructor]
    pub fn serve(key: Arc<SimulatedKey>, dir: String, port: u16) -> Result<Arc<Self>, PawError> {
        Self::serve_with(Path::new(&dir), fixture_options(&key), port)
    }

    /// The address to type at setup's "Where is your password store?".
    pub fn address(&self) -> String {
        format!("ssh://{USER}@127.0.0.1:{}/{REPO}", self.port)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Refuse every key from now on (as if it were never added to the
    /// account), or accept the authorised ones again.
    pub fn set_refuse_key(&self, refuse: bool) {
        self.shared.refuse_key.store(refuse, Ordering::SeqCst);
    }

    /// Present another host key on new connections, or the usual one again.
    pub fn set_changed_host_key(&self, changed: bool) {
        self.shared.changed_host_key.store(changed, Ordering::SeqCst);
    }

    /// Drop the whole connection halfway through every pack it sends, or
    /// stop doing so.
    pub fn set_drop_mid_clone(&self, drop: bool) {
        self.shared.drop_mid_pack.store(drop, Ordering::SeqCst);
    }

    /// Refuse every branch update from now on, as a server-side hook
    /// would (the pack is still received), or accept them again.
    pub fn set_refuse_pushes(&self, refuse: bool) {
        self.shared.refuse_pushes.store(refuse, Ordering::SeqCst);
    }

    /// Answer every git command as GitHub does for a repository the
    /// logged-in key's account cannot see ("ERROR: Repository not found."
    /// on stderr, exit status 1), or serve the repository again.
    pub fn set_repository_not_found(&self, not_found: bool) {
        self.shared.repository_not_found.store(not_found, Ordering::SeqCst);
    }

    /// Successful logins so far: each one cost the key one signature.
    pub fn logins(&self) -> u32 {
        self.shared.logins.load(Ordering::SeqCst)
    }

    pub fn tip(&self) -> Option<String> {
        let repo = Repository::open_bare(&self.shared.repo).ok()?;
        repo.refname_to_id(BRANCH).ok().map(|id| id.to_string())
    }

    /// Commits a file to `main` as another machine would.
    pub fn commit_file(&self, path: String, contents: Vec<u8>, message: String) -> Result<String, PawError> {
        commit_files(&self.shared.repo, &[(path, contents)], &message).map(|id| id.to_string())
    }
}

impl SimRemote {
    /// As `start`, listening on `port` (0: any free port).
    pub fn start_on(key: Arc<SimulatedKey>, port: u16) -> Result<Arc<Self>, PawError> {
        Self::start_with_on(fixture_options(&key), port)
    }

    /// The `SHA256:...` fingerprint of the host key it presents now, as
    /// setup's `ConfirmHost` shows it.
    pub fn host_key_fingerprint(&self) -> String {
        let line = if self.shared.changed_host_key.load(Ordering::SeqCst) {
            changed_host_key()
        } else {
            ed25519_host_key()
        };
        PublicKey::from_openssh(&line)
            .map(|key| key.fingerprint(HashAlg::Sha256).to_string())
            .unwrap_or_default()
    }
}

impl Drop for SimRemote {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
        if !self.keep_root {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

struct Connection {
    shared: Arc<Shared>,
    /// Index of this connection in `Shared::services`, once logged in.
    login: Option<usize>,
    channels: Vec<Channel<Msg>>,
}

impl Connection {
    fn decide(&self, key: &PublicKey) -> Auth {
        let known = locked(&self.shared.authorised).iter().any(|k| k.key_data() == key.key_data());
        if known && !self.shared.refuse_key.load(Ordering::SeqCst) { Auth::Accept } else { Auth::reject() }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Service {
    UploadPack,
    ReceivePack,
}

impl Service {
    fn name(self) -> &'static str {
        match self {
            Self::UploadPack => "git-upload-pack",
            Self::ReceivePack => "git-receive-pack",
        }
    }

    /// The service a command asks for: exactly what a git client sends,
    /// `git-upload-pack 'store.git'`, or `'/store.git'` for an `ssh://`
    /// address.
    fn from_command(command: &str) -> Option<Self> {
        [Self::UploadPack, Self::ReceivePack].into_iter().find(|service| {
            let name = service.name();
            command == format!("{name} '{REPO}'") || command == format!("{name} '/{REPO}'")
        })
    }
}

impl server::Handler for Connection {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        offer: server::PublicKeyOffer<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(self.decide(offer.public_key))
    }

    async fn auth_publickey(&mut self, _user: &str, key: server::VerifiedPublicKey<'_>) -> Result<Auth, Self::Error> {
        let decision = self.decide(key.public_key);
        if matches!(decision, Auth::Accept) {
            self.shared.logins.fetch_add(1, Ordering::SeqCst);
            let mut services = locked(&self.shared.services);
            self.login = Some(services.len());
            services.push(Vec::new());
        }
        Ok(decision)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.push(channel);
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(&mut self, id: ChannelId, command: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        let service = Service::from_command(&String::from_utf8_lossy(command));
        let channel = self.channels.iter().position(|c| c.id() == id).map(|at| self.channels.swap_remove(at));
        let (Some(service), Some(channel), Some(login)) = (service, channel, self.login) else {
            session.channel_failure(id)?;
            return Ok(());
        };
        if let Some(services) = locked(&self.shared.services).get_mut(login) {
            services.push(service.name().to_owned());
        }
        if service == Service::ReceivePack {
            let before = locked(&self.shared.before_push).take();
            if let Some(before) = before {
                before();
            }
        }
        session.channel_success(id)?;
        if self.shared.repository_not_found.load(Ordering::SeqCst) {
            tokio::spawn(not_found(channel));
        } else {
            tokio::spawn(serve(self.shared.clone(), service, channel, session.handle()));
        }
        Ok(())
    }
}

enum Ended {
    Done,
    Dropped,
}

async fn serve(shared: Arc<Shared>, service: Service, channel: Channel<Msg>, handle: server::Handle) {
    let (read, write) = channel.split();
    let mut input = Input { channel: read, buffer: Vec::new() };
    let mut out = write.make_writer();
    let result = match service {
        Service::UploadPack => upload_pack(&shared, &mut input, &mut out, &handle).await,
        Service::ReceivePack => receive_pack(&shared, &mut input, &mut out).await,
    };
    let status = match result {
        Ok(Ended::Dropped) => return,
        Ok(Ended::Done) => 0,
        Err(message) => {
            let mut err = write.make_writer_ext(Some(1));
            let _ = err.write_all(format!("paw-sim: {message}\n").as_bytes()).await;
            let _ = err.flush().await;
            1
        }
    };
    let _ = out.flush().await;
    let _ = write.exit_status(status).await;
    let _ = write.eof().await;
    let _ = write.close().await;
}

/// What GitHub answers a logged-in key whose account cannot see the
/// repository: a line on stderr and exit status 1, with nothing on stdout.
async fn not_found(channel: Channel<Msg>) {
    let (_read, write) = channel.split();
    let mut err = write.make_writer_ext(Some(1));
    let _ = err.write_all(b"ERROR: Repository not found.\n").await;
    let _ = err.flush().await;
    let _ = write.exit_status(1).await;
    let _ = write.eof().await;
    let _ = write.close().await;
}

struct Input {
    channel: ChannelReadHalf,
    buffer: Vec<u8>,
}

pub(crate) enum Pkt {
    Flush,
    Line(Vec<u8>),
}

/// The pkt-line at the start of `buffer` and how many bytes it takes, or
/// `None` if `buffer` does not hold all of it yet. A length under 4 is a
/// flush (0000) or, in newer protocols, a delimiter.
pub(crate) fn split_pkt(buffer: &[u8]) -> Result<Option<(Pkt, usize)>, String> {
    let Some(header) = buffer.get(..4) else { return Ok(None) };
    let length = std::str::from_utf8(header)
        .ok()
        .and_then(|hex| usize::from_str_radix(hex, 16).ok())
        .ok_or("not a pkt-line")?;
    if length < 4 {
        return Ok(Some((Pkt::Flush, 4)));
    }
    Ok(buffer.get(4..length).map(|line| (Pkt::Line(line.to_vec()), length)))
}

impl Input {
    /// Reads more from the client. `false` once it has closed its side.
    async fn fill(&mut self) -> bool {
        loop {
            match self.channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    self.buffer.extend_from_slice(&data);
                    return true;
                }
                Some(ChannelMsg::Eof | ChannelMsg::Close) | None => return false,
                Some(_) => {}
            }
        }
    }

    /// The next pkt-line, or `None` if the client closed before sending one.
    async fn pkt(&mut self) -> Result<Option<Pkt>, String> {
        loop {
            if let Some((pkt, used)) = split_pkt(&self.buffer)? {
                self.buffer.drain(..used);
                return Ok(Some(pkt));
            }
            if !self.fill().await {
                return if self.buffer.is_empty() { Ok(None) } else { Err("the client stopped mid-line".into()) };
            }
        }
    }
}

pub(crate) fn pkt_line(text: &str) -> Vec<u8> {
    let mut out = format!("{:04x}", text.len() + 4).into_bytes();
    out.extend_from_slice(text.as_bytes());
    out
}

pub(crate) const FLUSH: &[u8] = b"0000";

async fn send(out: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> Result<(), String> {
    out.write_all(bytes).await.map_err(|e| e.to_string())?;
    out.flush().await.map_err(|e| e.to_string())
}

pub(crate) type Branches = (Vec<(Oid, String)>, Option<String>);

/// The branches, as (commit, name), sorted by name; and where `HEAD` points
/// if that branch exists.
pub(crate) fn branches(repo: &Path) -> Result<Branches, String> {
    let repo = Repository::open_bare(repo).map_err(|e| e.message().to_owned())?;
    let mut found = Vec::new();
    for reference in repo.references_glob("refs/heads/*").map_err(|e| e.message().to_owned())? {
        let reference = reference.map_err(|e| e.message().to_owned())?;
        if let (Ok(name), Some(id)) = (reference.name(), reference.target()) {
            found.push((id, name.to_owned()));
        }
    }
    found.sort_by(|a, b| a.1.cmp(&b.1));
    let head = repo
        .find_reference("HEAD")
        .ok()
        .and_then(|head| head.symbolic_target().ok().flatten().map(str::to_owned))
        .filter(|target| found.iter().any(|(_, name)| name == target));
    Ok((found, head))
}

/// The ref advertisement: the first line carries the capabilities.
pub(crate) fn advertisement(refs: &[(Oid, String)], head: Option<&str>, capabilities: &str) -> Vec<u8> {
    let mut lines: Vec<(Oid, String)> = Vec::new();
    let mut capabilities = capabilities.to_owned();
    if let Some(head) = head
        && let Some((id, _)) = refs.iter().find(|(_, name)| name == head)
    {
        lines.push((*id, "HEAD".into()));
        capabilities = format!("{capabilities} symref=HEAD:{head}");
    }
    lines.extend(refs.iter().cloned());
    let mut out = Vec::new();
    if lines.is_empty() {
        out.extend(pkt_line(&format!("{ZERO_HEX} capabilities^{{}}\0{capabilities}\n")));
    }
    for (index, (id, name)) in lines.iter().enumerate() {
        if index == 0 {
            out.extend(pkt_line(&format!("{id} {name}\0{capabilities}\n")));
        } else {
            out.extend(pkt_line(&format!("{id} {name}\n")));
        }
    }
    out.extend_from_slice(FLUSH);
    out
}

/// The commit a `want <id> [capabilities]` line asks for, or the line's
/// `have <id>`, by `word`.
pub(crate) fn negotiation_oid(line: &str, word: &str) -> Result<Oid, String> {
    let rest = line.strip_prefix(word).and_then(|rest| rest.strip_prefix(' '));
    let rest = rest.ok_or_else(|| format!("unsupported request: {line}"))?;
    parse_oid(rest.split(' ').next().unwrap_or_default())
}

pub(crate) fn parse_oid(text: &str) -> Result<Oid, String> {
    if text.len() != 40 {
        return Err(format!("not an object id: {text}"));
    }
    Oid::from_str(text).map_err(|_| format!("not an object id: {text}"))
}

pub(crate) fn known_commits(repo: &Path, haves: &[Oid]) -> Result<Vec<Oid>, String> {
    let repo = Repository::open_bare(repo).map_err(|e| e.message().to_owned())?;
    Ok(haves.iter().copied().filter(|id| repo.find_commit(*id).is_ok()).collect())
}

/// A pack of everything reachable from `wants` and not from `common`.
pub(crate) fn build_pack(repo: &Path, wants: &[Oid], common: &[Oid]) -> Result<Vec<u8>, String> {
    let message = |e: git2::Error| e.message().to_owned();
    let repo = Repository::open_bare(repo).map_err(message)?;
    let mut walk = repo.revwalk().map_err(message)?;
    for want in wants {
        walk.push(*want).map_err(message)?;
    }
    for have in common {
        walk.hide(*have).map_err(message)?;
    }
    let mut builder = repo.packbuilder().map_err(message)?;
    builder.insert_walk(&mut walk).map_err(message)?;
    let mut buffer = git2::Buf::new();
    builder.write_buf(&mut buffer).map_err(message)?;
    Ok(buffer.to_vec())
}

async fn upload_pack(
    shared: &Shared,
    input: &mut Input,
    out: &mut (impl AsyncWrite + Unpin),
    handle: &server::Handle,
) -> Result<Ended, String> {
    let (refs, head) = branches(&shared.repo)?;
    send(out, &advertisement(&refs, head.as_deref(), &format!("ofs-delta {AGENT}"))).await?;

    // Wants, up to a flush. A client with nothing to fetch says nothing, or
    // only flushes.
    let mut wants = Vec::new();
    loop {
        match input.pkt().await? {
            None => return Ok(Ended::Done),
            Some(Pkt::Flush) => break,
            Some(Pkt::Line(line)) => {
                let id = negotiation_oid(String::from_utf8_lossy(&line).trim_end(), "want")?;
                if !refs.iter().any(|(tip, _)| *tip == id) {
                    return Err(format!("not our ref {id}"));
                }
                wants.push(id);
            }
        }
    }
    if wants.is_empty() {
        return Ok(Ended::Done);
    }

    // Haves until "done". Without multi_ack, libgit2 reads one ACK or NAK
    // per flush and one after "done", stopping at the first ACK.
    let mut haves = Vec::new();
    loop {
        let reply = match input.pkt().await? {
            None => return Err("the client left during negotiation".into()),
            Some(Pkt::Flush) => false,
            Some(Pkt::Line(line)) => {
                let line = String::from_utf8_lossy(&line);
                let line = line.trim_end();
                if line == "done" {
                    true
                } else {
                    haves.push(negotiation_oid(line, "have")?);
                    continue;
                }
            }
        };
        let common = known_commits(&shared.repo, &haves)?;
        let answer = match common.first() {
            Some(id) => format!("ACK {id}\n"),
            None => "NAK\n".to_owned(),
        };
        send(out, &pkt_line(&answer)).await?;
        if reply {
            let pack = build_pack(&shared.repo, &wants, &common)?;
            if shared.drop_mid_pack.load(Ordering::SeqCst) {
                send(out, &pack[..pack.len() / 2]).await?;
                let _ = handle.disconnect(Disconnect::ByApplication, "dropped on purpose".into(), "en".into()).await;
                return Ok(Ended::Dropped);
            }
            send(out, &pack).await?;
            return Ok(Ended::Done);
        }
    }
}

/// Finds where a pack that starts at the beginning of the buffer ends,
/// without trusting anything in it: each object's zlib stream is inflated
/// to find its end. Keeps its place between calls.
struct PackScanner {
    objects: Option<u32>,
    scanned: u32,
    at: usize,
}

impl PackScanner {
    fn new() -> Self {
        Self { objects: None, scanned: 0, at: 12 }
    }

    /// `Some(length)` once the whole pack is in `buffer`; `None` if more is
    /// needed.
    fn scan(&mut self, buffer: &[u8]) -> Result<Option<usize>, String> {
        let objects = match self.objects {
            Some(objects) => objects,
            None => {
                let Some(header) = buffer.get(..12) else { return Ok(None) };
                if &header[..4] != b"PACK" || !matches!(header[4..8], [0, 0, 0, 2] | [0, 0, 0, 3]) {
                    return Err("not a pack".into());
                }
                let objects = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
                self.objects = Some(objects);
                objects
            }
        };
        while self.scanned < objects {
            let Some(length) = Self::object(&buffer[self.at.min(buffer.len())..])? else { return Ok(None) };
            self.at += length;
            self.scanned += 1;
        }
        let end = self.at + 20;
        Ok((buffer.len() >= end).then_some(end))
    }

    /// The length of the object at the start of `bytes`, if all of it is
    /// there.
    fn object(bytes: &[u8]) -> Result<Option<usize>, String> {
        let mut at = 0;
        let Some(&first) = bytes.first() else { return Ok(None) };
        let kind = (first >> 4) & 7;
        let mut byte = first;
        while byte & 0x80 != 0 {
            at += 1;
            let Some(&next) = bytes.get(at) else { return Ok(None) };
            byte = next;
        }
        at += 1;
        match kind {
            1..=4 => {}
            // OFS_DELTA: a variable-length offset back.
            6 => loop {
                let Some(&next) = bytes.get(at) else { return Ok(None) };
                at += 1;
                if next & 0x80 == 0 {
                    break;
                }
            },
            // REF_DELTA: the base's object id.
            7 => at += 20,
            _ => return Err("a pack object of an unknown type".into()),
        }
        let Some(data) = bytes.get(at..) else { return Ok(None) };
        let mut inflate = flate2::Decompress::new(true);
        let mut scratch = [0u8; 16 * 1024];
        loop {
            let consumed = inflate.total_in() as usize;
            let before = (inflate.total_in(), inflate.total_out());
            let status = inflate
                .decompress(&data[consumed..], &mut scratch, flate2::FlushDecompress::None)
                .map_err(|e| format!("a damaged pack: {e}"))?;
            if status == flate2::Status::StreamEnd {
                return Ok(Some(at + inflate.total_in() as usize));
            }
            if (inflate.total_in(), inflate.total_out()) == before {
                return Ok(None);
            }
        }
    }
}

/// One `old new ref` line of a push.
struct Command {
    old: Option<Oid>,
    new: Option<Oid>,
    name: String,
}

fn optional_oid(text: &str) -> Result<Option<Oid>, String> {
    if text == ZERO_HEX { Ok(None) } else { parse_oid(text).map(Some) }
}

fn apply_push(repo: &Path, pack: Option<&[u8]>, commands: &[Command], refuse: bool) -> Result<Vec<String>, String> {
    let message = |e: git2::Error| e.message().to_owned();
    let repo = Repository::open_bare(repo).map_err(message)?;
    if let Some(pack) = pack.filter(|pack| pack.get(8..12) != Some(&[0, 0, 0, 0][..])) {
        use std::io::Write;
        let odb = repo.odb().map_err(message)?;
        let mut writer = odb.packwriter().map_err(message)?;
        writer.write_all(pack).map_err(|e| e.to_string())?;
        writer.commit().map_err(message)?;
    }
    let mut report = Vec::new();
    for command in commands {
        let name = &command.name;
        let refused = |why: &str| format!("ng {name} {why}\n");
        let line = if refuse {
            refused("pre-receive hook declined")
        } else if !name.starts_with("refs/heads/") || !git2::Reference::is_valid_name(name) {
            refused("only branches can be pushed here")
        } else if let Some(new) = command.new {
            let current = repo.refname_to_id(name).ok();
            if current != command.old {
                refused("fetch first")
            } else if repo.find_commit(new).is_err() {
                refused("missing objects")
            } else if command.old.is_some_and(|old| old != new && !repo.graph_descendant_of(new, old).unwrap_or(false)) {
                refused("non-fast-forward")
            } else {
                match repo.reference(name, new, true, "push") {
                    Ok(_) => format!("ok {name}\n"),
                    Err(e) => refused(e.message()),
                }
            }
        } else {
            refused("deleting branches is not allowed here")
        };
        report.push(line);
    }
    Ok(report)
}

async fn receive_pack(shared: &Shared, input: &mut Input, out: &mut (impl AsyncWrite + Unpin)) -> Result<Ended, String> {
    let (refs, _) = branches(&shared.repo)?;
    send(out, &advertisement(&refs, None, &format!("report-status ofs-delta {AGENT}"))).await?;

    let mut commands = Vec::new();
    let mut report_status = false;
    loop {
        match input.pkt().await? {
            None => return Ok(Ended::Done),
            Some(Pkt::Flush) => break,
            Some(Pkt::Line(line)) => {
                let (command, capabilities) = match line.iter().position(|b| *b == 0) {
                    Some(nul) => (&line[..nul], String::from_utf8_lossy(&line[nul + 1..]).into_owned()),
                    None => (&line[..], String::new()),
                };
                report_status |= capabilities.split(' ').any(|c| c.trim() == "report-status");
                let command = String::from_utf8_lossy(command);
                let mut fields = command.trim_end().splitn(3, ' ');
                let (Some(old), Some(new), Some(name)) = (fields.next(), fields.next(), fields.next()) else {
                    return Err(format!("not a push command: {}", command.trim_end()));
                };
                commands.push(Command { old: optional_oid(old)?, new: optional_oid(new)?, name: name.to_owned() });
            }
        }
    }
    if commands.is_empty() {
        return Ok(Ended::Done);
    }

    let pack = if commands.iter().any(|c| c.new.is_some()) {
        let mut scanner = PackScanner::new();
        loop {
            if let Some(end) = scanner.scan(&input.buffer)? {
                break Some(input.buffer.drain(..end).collect::<Vec<u8>>());
            }
            if input.buffer.len() > MAX_PUSH {
                return Err("the push is too large".into());
            }
            if !input.fill().await {
                return Err("the client stopped mid-pack".into());
            }
        }
    } else {
        None
    };

    let (unpack, lines) = match apply_push(&shared.repo, pack.as_deref(), &commands, shared.refuse_pushes.load(Ordering::SeqCst)) {
        Ok(lines) => ("unpack ok\n".to_owned(), lines),
        Err(why) => {
            let lines = commands.iter().map(|c| format!("ng {} unpacker error\n", c.name)).collect();
            (format!("unpack {why}\n"), lines)
        }
    };
    if report_status {
        let mut reply = pkt_line(&unpack);
        for line in &lines {
            reply.extend(pkt_line(line));
        }
        reply.extend_from_slice(FLUSH);
        send(out, &reply).await?;
    }
    Ok(Ended::Done)
}

/// The git steps of a sync one at a time, for tests that look at each:
/// test support, not how the app syncs (it runs whole syncs as key tasks,
/// `StoreSync.send_with_key`). Not in device builds.
pub mod steps {
    use crate::lock::StoreDirs;
    use crate::files::refs;
    use crate::key::SshSigner;
    use crate::sync::operation::{self, SyncReport};
    use crate::fault::Fault;
    use crate::sync::{self, Cloned, Connection, OpenStore, Remote, SyncError, git_err};
    use crate::write::{Author, TreeEdit, commit_edits};

    /// Clones the remote's default branch into the store's repository,
    /// which must be empty or not exist yet, as setup's download does.
    pub fn clone(connection: &Connection, remote: &Remote, store: &StoreDirs) -> Result<Cloned, SyncError> {
        Ok(sync::clone_with_transfer(connection, remote, store, None)?)
    }

    /// Updates `origin/*` from the remote and returns the new tip of the
    /// current branch's remote counterpart. The working copy is not changed.
    pub fn fetch(connection: &Connection, store: &StoreDirs) -> Result<String, SyncError> {
        let store = OpenStore::open(store)?;
        let (_, _held) = sync::fetch_branches(connection, &store, None)?;
        let (_, short) = sync::current_branch(&store.repo)?;
        Ok(store.repo.refname_to_id(&refs::remote_tracking(&short)).map(|id| id.to_string()).map_err(git_err)?)
    }

    /// Commits `contents` as the file `path` (relative to the store, `/`
    /// separated) on the current branch, and nothing else, as a save does
    /// once it has encrypted the entry.
    pub fn commit(store: &StoreDirs, path: &str, contents: &[u8], message: &str, author: &Author) -> Result<String, Fault> {
        let held = store.exclusive()?;
        commit_edits(&held, &OpenStore::open(store)?, &[TreeEdit::Write { path, contents }], message, author)
    }

    /// Integrates `origin/<branch>` into the current branch, as every
    /// sync does after its fetch.
    pub fn integrate(store: &StoreDirs) -> Result<sync::Integration, SyncError> {
        let held = store.exclusive()?;
        let store = OpenStore::open(store)?;
        sync::integrate::apply(&held, &store, sync::integrate::plan(&held, &store)?)
    }

    /// Pushes the current branch to the branch of the same name, as a
    /// fast-forward only, as a send does.
    pub fn push(connection: &Connection, store: &StoreDirs) -> Result<(), sync::PushError> {
        push_recording_within(connection, store, crate::lock::RECORD_WAIT)
    }

    /// [`push`], waiting at most `record_wait` to record a push that landed.
    pub fn push_recording_within(
        connection: &Connection,
        store: &StoreDirs,
        record_wait: std::time::Duration,
    ) -> Result<(), sync::PushError> {
        let pushing = store.lock().pushing(crate::lock::SEND_WAIT)?;
        let store = OpenStore::open(store)?;
        let held = store.dirs.exclusive()?;
        sync::push_tip(connection, &store, held, &pushing, record_wait)
    }

    /// A whole sync of `store` on one login, as `StoreSync`'s key tasks run
    /// it: a send if `send`, else a refresh.
    pub fn sync(
        remote: &Remote,
        signer: &dyn SshSigner,
        store: &StoreDirs,
        send: bool,
    ) -> Result<SyncReport, crate::task::TaskFailure> {
        operation::run_until(remote, signer, store, crate::write::held_back::holds_back, send, &|| false, None)
    }
}

//! A git host for the sync tests: an in-process SSH server that runs the
//! real `git-upload-pack` and `git-receive-pack` against a bare repository
//! in a temporary directory. Nothing leaves the machine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use paw_core::sync::{HostKeyPin, Remote};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId, ChannelMsg};
use tokio::io::AsyncWriteExt;

const REPO: &str = "store.git";

/// Runs `git` with the developer's own configuration out of the picture, so
/// the tests neither depend on it nor sign anything with a real key.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = isolated(Command::new("git"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git is installed");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn isolated(mut command: Command) -> Command {
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Desktop")
        .env("GIT_AUTHOR_EMAIL", "desktop@example.com")
        .env("GIT_COMMITTER_NAME", "Desktop")
        .env("GIT_COMMITTER_EMAIL", "desktop@example.com");
    command
}

fn host_key(seed: u8) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
}

/// A public key line for a host key the server does not hold.
pub fn some_other_host_key() -> String {
    host_key(2).public_key().to_openssh().unwrap()
}

/// What the server saw, for the tests' assertions about connection reuse.
#[derive(Default)]
struct Seen {
    logins: AtomicU32,
    /// One entry per successful login: the git services run on it, in order.
    services: Mutex<Vec<Vec<String>>>,
}

pub struct GitServer {
    // Dropping the runtime stops the server.
    _runtime: tokio::runtime::Runtime,
    root: tempfile::TempDir,
    port: u16,
    seen: Arc<Seen>,
}

impl GitServer {
    /// Starts a server holding one bare repository with a single commit that
    /// adds `README`. Only the keys in `authorised` (public key lines) may
    /// log in.
    pub fn start(authorised: &[String]) -> Self {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "--quiet", "--bare", "--initial-branch=main", REPO]);

        let seen = Arc::new(Seen::default());
        let template = Connection {
            repo: root.path().join(REPO),
            authorised: authorised
                .iter()
                .map(|line| PublicKey::from_openssh(line).unwrap())
                .collect(),
            seen: seen.clone(),
            login: None,
            channels: HashMap::new(),
        };
        let config = Arc::new(server::Config {
            keys: vec![host_key(1)],
            // Refusals are delayed by default to slow down guessing.
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind(("127.0.0.1", 0)))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        runtime.spawn(async move {
            let mut server = Listener(template);
            server.run_on_socket(config, &listener).await.unwrap();
        });

        let server = Self { _runtime: runtime, root, port, seen };
        server.desktop_commit("README", "a password store\n");
        server
    }

    pub fn remote(&self) -> Remote {
        Remote {
            host: "127.0.0.1".into(),
            port: self.port,
            user: "git".into(),
            path: REPO.into(),
            host_key: HostKeyPin::parse(&host_key(1).public_key().to_openssh().unwrap()).unwrap(),
        }
    }

    pub fn bare(&self) -> PathBuf {
        self.root.path().join(REPO)
    }

    /// The commit `main` points at on the server.
    pub fn tip(&self) -> String {
        git(&self.bare(), &["rev-parse", "refs/heads/main"])
    }

    /// Adds a commit to the server's `main` the way another machine would:
    /// from a separate clone, over the file system.
    pub fn desktop_commit(&self, path: &str, contents: &str) -> String {
        self.desktop_change(path, |file| std::fs::write(file, contents).unwrap())
    }

    /// Commits a symbolic link named `path` that points at `target`, as a
    /// hostile or careless collaborator could.
    pub fn desktop_symlink(&self, path: &str, target: &Path) -> String {
        self.desktop_change(path, |link| std::os::unix::fs::symlink(target, link).unwrap())
    }

    fn desktop_change(&self, path: &str, create: impl FnOnce(&Path)) -> String {
        let work = tempfile::tempdir().unwrap();
        let bare = self.bare();
        git(work.path(), &["clone", "--quiet", bare.to_str().unwrap(), "."]);
        create(&work.path().join(path));
        git(work.path(), &["add", path]);
        git(work.path(), &["commit", "--quiet", "-m", &format!("Add {path} from the desktop.")]);
        git(work.path(), &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
        self.tip()
    }

    pub fn logins(&self) -> u32 {
        self.seen.logins.load(Ordering::SeqCst)
    }

    /// The git services each logged-in connection ran, in login order.
    pub fn services(&self) -> Vec<Vec<String>> {
        self.seen.services.lock().unwrap().clone()
    }
}

struct Listener(Connection);

impl server::Server for Listener {
    type Handler = Connection;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Connection {
        Connection {
            repo: self.0.repo.clone(),
            authorised: self.0.authorised.clone(),
            seen: self.0.seen.clone(),
            login: None,
            channels: HashMap::new(),
        }
    }
}

struct Connection {
    repo: PathBuf,
    authorised: Vec<PublicKey>,
    seen: Arc<Seen>,
    /// Index of this connection in `Seen::services`, once logged in.
    login: Option<usize>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl Connection {
    fn decide(&self, key: &PublicKey) -> Auth {
        if self.authorised.iter().any(|k| k.key_data() == key.key_data()) {
            Auth::Accept
        } else {
            Auth::reject()
        }
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

    async fn auth_publickey(
        &mut self,
        _user: &str,
        key: server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        let decision = self.decide(key.public_key);
        if matches!(decision, Auth::Accept) {
            self.seen.logins.fetch_add(1, Ordering::SeqCst);
            let mut services = self.seen.services.lock().unwrap();
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
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        id: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Exactly what a git client sends: `git-upload-pack 'store.git'`.
        let command = String::from_utf8_lossy(command);
        let service = ["git-upload-pack", "git-receive-pack"]
            .into_iter()
            .find(|service| command == format!("{service} '{REPO}'"));
        let (Some(service), Some(channel), Some(login)) =
            (service, self.channels.remove(&id), self.login)
        else {
            session.channel_failure(id)?;
            return Ok(());
        };
        self.seen.services.lock().unwrap()[login].push(service.to_owned());
        session.channel_success(id)?;

        let mut git = isolated(Command::new("git"));
        git.arg(service.strip_prefix("git-").unwrap())
            .arg(&self.repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        tokio::spawn(serve(tokio::process::Command::from(git), channel));
        Ok(())
    }
}

/// Connects one git service process to one SSH channel until it exits.
async fn serve(mut git: tokio::process::Command, channel: Channel<Msg>) {
    let mut child = git.spawn().expect("git is installed");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (mut from_client, to_client) = channel.split();

    let input = tokio::spawn(async move {
        while let Some(message) = from_client.wait().await {
            match message {
                ChannelMsg::Data { data } => {
                    if stdin.write_all(&data).await.is_err() {
                        break;
                    }
                }
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
    });

    let mut out = to_client.make_writer();
    let mut err = to_client.make_writer_ext(Some(1));
    let _ = tokio::join!(
        tokio::io::copy(&mut stdout, &mut out),
        tokio::io::copy(&mut stderr, &mut err),
    );
    let status = child.wait().await.unwrap();
    input.abort();

    let _ = to_client.exit_status(status.code().unwrap_or(1) as u32).await;
    let _ = to_client.eof().await;
    let _ = to_client.close().await;
}

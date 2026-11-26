//! A simulated GitHub: an HTTP server on a loopback port with the parts of
//! github.com and api.github.com paw uses, for tests and for the app's UI
//! tests in the iOS simulator.
//!
//! - the device flow (`/login/device/code`, `/login/oauth/access_token`)
//!   with outcomes the test chooses: pending a number of times, `slow_down`,
//!   denied, expired, or waiting until told the person authorised;
//! - expiring tokens with refresh and rotation (each refresh token works
//!   once and takes its access token with it), and revocation;
//! - `/user/installations`, `/user/installations/{id}/repositories` with
//!   paging, `/credentials/revoke`;
//! - git's smart HTTP (`info/refs`, `git-upload-pack`; no push) serving the
//!   repository a [`SimRemote`] serves over SSH, with basic auth
//!   `x-access-token:<token>`.
//!
//! Plain HTTP: paw allows it only for endpoints made with
//! `Endpoints::simulated`, which exists only with the `sim` feature.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine;
use git2::Oid;
use serde_json::json;

use super::remote::{
    AGENT, FLUSH, Pkt, SimRemote, advertisement, branches, build_pack, known_commits, parse_oid, pkt_line, split_pkt,
};
use crate::PawError;
use crate::github::GitHubAccount;
use crate::github::device::{Cancel, PollClock};
use crate::github::session::{TokenVault, VaultError};

/// The client id the simulated GitHub expects unless told otherwise. Not
/// paw's real one, so a test can tell which was sent.
pub const SIM_CLIENT_ID: &str = "Iv1.simulated-paw";
pub const SIM_USER_CODE: &str = "WDJB-MJHT";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

fn failed(what: &str, error: impl std::fmt::Display) -> PawError {
    PawError::Git(format!("simulated GitHub: could not {what}: {error}"))
}

fn random_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut raw = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone)]
pub struct SimHttpRequest {
    pub method: String,
    /// Without the query.
    pub path: String,
    pub query: String,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl SimHttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// Whether `secret` appears anywhere in the request: line, headers
    /// (also inside a basic-auth header) or body.
    pub fn contains(&self, secret: &str) -> bool {
        let basic = self
            .header("authorization")
            .and_then(|v| v.strip_prefix("Basic "))
            .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
            .map(|d| String::from_utf8_lossy(&d).into_owned())
            .unwrap_or_default();
        self.path.contains(secret)
            || self.query.contains(secret)
            || self.headers.iter().any(|(_, v)| v.contains(secret))
            || basic.contains(secret)
            || String::from_utf8_lossy(&self.body).contains(secret)
    }

    fn form(&self) -> HashMap<String, String> {
        let text = String::from_utf8_lossy(&self.body);
        text.split('&')
            .chain(self.query.split('&'))
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (decode(k), decode(v)))
            .collect()
    }

    fn query_value(&self, name: &str) -> Option<String> {
        self.query.split('&').filter_map(|p| p.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| decode(v))
    }
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A gate that packs wait at, halfway through, while it is armed: a fetch
/// stalled mid-transfer, for tests that check what waits for it.
#[derive(Default)]
struct Stall {
    state: Mutex<StallState>,
    changed: Condvar,
}

#[derive(Default)]
struct StallState {
    armed: bool,
    waiting: u32,
}

impl Stall {
    /// Waits while armed, at most 30 seconds.
    fn wait(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut state = lock(&self.state);
        state.waiting += 1;
        self.changed.notify_all();
        while state.armed {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            state = self.changed.wait_timeout(state, deadline - now).unwrap_or_else(|p| p.into_inner()).0;
        }
        state.waiting -= 1;
        self.changed.notify_all();
    }
}

struct Reply {
    status: u16,
    content_type: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// Send only this many bytes of the body, then close.
    cut_at: Option<usize>,
    /// Send half the body, wait here, then the rest.
    stall: Option<Arc<Stall>>,
}

impl Reply {
    fn json(status: u16, value: serde_json::Value) -> Self {
        Self { status, content_type: "application/json; charset=utf-8".into(), headers: Vec::new(), body: value.to_string().into_bytes(), cut_at: None, stall: None }
    }

    fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self { status, content_type: content_type.into(), headers: Vec::new(), body, cut_at: None, stall: None }
    }

    fn status(status: u16) -> Self {
        Self::json(status, json!({ "message": "simulated" }))
    }
}

type Handler = Arc<dyn Fn(&SimHttpRequest) -> Reply + Send + Sync>;

struct Server {
    port: u16,
    stopped: Arc<AtomicBool>,
}

impl Server {
    fn start(record: Arc<Mutex<Vec<SimHttpRequest>>>, handler: Handler) -> Result<Self, PawError> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| failed("listen", e))?;
        let port = listener.local_addr().map_err(|e| failed("listen", e))?.port();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let record = record.clone();
                let handler = handler.clone();
                std::thread::spawn(move || {
                    let _ = serve_one(stream, &record, &handler);
                });
            }
        });
        Ok(Self { port, stopped })
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        // Wakes the accept loop so it sees the flag.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn serve_one(stream: TcpStream, record: &Mutex<Vec<SimHttpRequest>>, handler: &Handler) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let mut parts = line.trim_end().splitn(3, ' ');
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let length = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length.min(16 << 20)];
    reader.read_exact(&mut body)?;
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let request = SimHttpRequest { method, path: path.to_owned(), query: query.to_owned(), headers, body };
    lock(record).push(request.clone());
    let reply = handler(&request);

    let mut out = stream;
    let mut head = format!(
        "HTTP/1.1 {} SIM\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.content_type,
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    out.write_all(head.as_bytes())?;
    let end = reply.cut_at.unwrap_or(reply.body.len()).min(reply.body.len());
    match &reply.stall {
        Some(stall) => {
            let half = end / 2;
            out.write_all(&reply.body[..half])?;
            out.flush()?;
            stall.wait();
            out.write_all(&reply.body[half..end])?;
        }
        None => out.write_all(&reply.body[..end])?,
    }
    out.flush()?;
    let _ = out.shutdown(Shutdown::Both);
    Ok(())
}

/// What the device flow does once a code is issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SimDeviceOutcome {
    /// `authorization_pending` this many times, then the tokens.
    AuthoriseAfter { pending: u32 },
    /// `slow_down` on the first poll, `authorization_pending` this many
    /// times, then the tokens.
    SlowDownThenAuthorise { pending: u32 },
    /// `authorization_pending` this many times, then `access_denied`.
    DenyAfter { pending: u32 },
    /// `authorization_pending` this many times, then `expired_token`.
    ExpireAfter { pending: u32 },
    /// `authorization_pending` until `authorise_pending` or `deny_pending`.
    WaitForPerson,
    /// The app's device flow is off: `device_flow_disabled`.
    Disabled,
}

struct Pending {
    device_code: String,
    polls: u32,
    decided: Option<bool>,
}

struct State {
    outcome: SimDeviceOutcome,
    pending: Option<Pending>,
    /// Access token → its refresh token.
    access: HashMap<String, Option<String>>,
    /// Refresh tokens that still work.
    refresh: Vec<String>,
    issued: u32,
    expiring: bool,
    installed: bool,
    extra_repositories: u32,
    redirect_git: Option<String>,
    drop_mid_pack: bool,
    revoked: Vec<String>,
    interval: u64,
    expires_in: u64,
}

struct Shared {
    bare: PathBuf,
    owner: String,
    name: String,
    ssh_address: String,
    client_id: Mutex<String>,
    state: Mutex<State>,
    web: Mutex<String>,
    stall: Arc<Stall>,
}

#[derive(uniffi::Object)]
pub struct SimGitHub {
    shared: Arc<Shared>,
    record: Arc<Mutex<Vec<SimHttpRequest>>>,
    server: Server,
}

const STORE_ID: u64 = 4_242;
const INSTALLATION_ID: u64 = 77;

impl SimGitHub {
    /// Serves the repository of `remote` as `repository` (`owner/name`).
    pub fn start_for(remote: &SimRemote, repository: &str) -> Result<Arc<Self>, PawError> {
        let (owner, name) = repository.split_once('/').ok_or_else(|| failed("start", "the repository is owner/name"))?;
        let shared = Arc::new(Shared {
            bare: remote.bare(),
            owner: owner.into(),
            name: name.into(),
            ssh_address: remote.address(),
            client_id: Mutex::new(SIM_CLIENT_ID.into()),
            state: Mutex::new(State {
                outcome: SimDeviceOutcome::AuthoriseAfter { pending: 0 },
                pending: None,
                access: HashMap::new(),
                refresh: Vec::new(),
                issued: 0,
                expiring: true,
                installed: true,
                extra_repositories: 0,
                redirect_git: None,
                drop_mid_pack: false,
                revoked: Vec::new(),
                interval: 5,
                expires_in: 900,
            }),
            web: Mutex::new(String::new()),
            stall: Arc::new(Stall::default()),
        });
        let record = Arc::new(Mutex::new(Vec::new()));
        let handling = shared.clone();
        let server = Server::start(record.clone(), Arc::new(move |request| handle(&handling, request)))?;
        *lock(&shared.web) = server.url();
        Ok(Arc::new(Self { shared, record, server }))
    }

    pub fn requests(&self) -> Vec<SimHttpRequest> {
        lock(&self.record).clone()
    }

    /// Every token issued that still works, access and refresh.
    pub fn live_tokens(&self) -> Vec<String> {
        let state = lock(&self.shared.state);
        state.access.keys().cloned().chain(state.refresh.iter().cloned()).collect()
    }

    pub fn all_tokens(&self) -> Vec<String> {
        let state = lock(&self.shared.state);
        let mut all: Vec<String> = state.access.keys().cloned().collect();
        all.extend(state.refresh.iter().cloned());
        all.extend(state.revoked.iter().cloned());
        all.extend(lock(&ISSUED).iter().filter(|(port, _)| *port == self.server.port).map(|(_, t)| t.clone()));
        all.sort();
        all.dedup();
        all
    }

    /// Tokens revoked through `/credentials/revoke`.
    pub fn revoked_tokens(&self) -> Vec<String> {
        lock(&self.shared.state).revoked.clone()
    }

    pub fn endpoints(&self) -> crate::github::Endpoints {
        let url = self.server.url();
        let store = format!("{}/{}", self.shared.owner, self.shared.name);
        crate::github::Endpoints::simulated(&url, &url, &lock(&self.shared.client_id), Some((store, self.shared.ssh_address.clone())))
    }

    /// Sets the poll interval and code lifetime the next code gets.
    pub fn set_timing(&self, interval: u64, expires_in: u64) {
        let mut state = lock(&self.shared.state);
        state.interval = interval;
        state.expires_in = expires_in;
    }
}

/// Every token any simulated GitHub issued, with its server's port.
static ISSUED: Mutex<Vec<(u16, String)>> = Mutex::new(Vec::new());

#[uniffi::export]
impl SimGitHub {
    /// A simulated GitHub serving `remote`'s repository as `repository`
    /// (`owner/name`), installed on it, authorising every sign-in at the
    /// first poll.
    #[uniffi::constructor]
    pub fn start(remote: Arc<SimRemote>, repository: String) -> Result<Arc<Self>, PawError> {
        Self::start_for(&remote, &repository)
    }

    /// The sign-in for the store in `state_dir`, kept in `vault`, against
    /// this simulated GitHub.
    pub fn open_account(&self, state_dir: String, vault: Arc<dyn TokenVault>) -> Arc<GitHubAccount> {
        Arc::new(GitHubAccount::with(
            self.endpoints(),
            std::path::Path::new(&state_dir),
            vault,
            Arc::new(crate::github::SystemPollClock::default()),
            crate::github::session::system_wall_clock(),
        ))
    }

    pub fn set_device_outcome(&self, outcome: SimDeviceOutcome) {
        lock(&self.shared.state).outcome = outcome;
    }

    /// The person typed the code and pressed Authorise.
    pub fn authorise_pending(&self) {
        if let Some(pending) = &mut lock(&self.shared.state).pending {
            pending.decided = Some(true);
        }
    }

    pub fn set_installed(&self, installed: bool) {
        lock(&self.shared.state).installed = installed;
    }

    /// Adds this many other repositories (not stores) to the installation.
    pub fn set_extra_repositories(&self, count: u32) {
        lock(&self.shared.state).extra_repositories = count;
    }

    /// The person revoked paw at GitHub: nothing issued works any more.
    pub fn revoke_authorisation(&self) {
        let mut state = lock(&self.shared.state);
        state.access.clear();
        state.refresh.clear();
    }
}

impl SimGitHub {
    /// `http://127.0.0.1:<port>`: both the web and the API address.
    pub fn url(&self) -> String {
        self.server.url()
    }

    pub fn client_id(&self) -> String {
        lock(&self.shared.client_id).clone()
    }

    /// Expect another client id (to test that paw sends its own).
    pub fn set_client_id(&self, client_id: String) {
        *lock(&self.shared.client_id) = client_id;
    }

    /// The person pressed Cancel.
    pub fn deny_pending(&self) {
        if let Some(pending) = &mut lock(&self.shared.state).pending {
            pending.decided = Some(false);
        }
    }

    pub fn has_pending_code(&self) -> bool {
        lock(&self.shared.state).pending.is_some()
    }

    pub fn set_expiring_tokens(&self, expiring: bool) {
        lock(&self.shared.state).expiring = expiring;
    }

    /// Every access token stops working; refresh tokens still work.
    pub fn expire_access_tokens(&self) {
        let mut state = lock(&self.shared.state);
        state.access.clear();
    }

    /// Git requests are redirected to `url` (another host), or not.
    pub fn set_redirect_git(&self, url: Option<String>) {
        lock(&self.shared.state).redirect_git = url;
    }

    pub fn set_drop_mid_pack(&self, drop: bool) {
        lock(&self.shared.state).drop_mid_pack = drop;
    }

    pub fn request_count(&self) -> u32 {
        lock(&self.record).len() as u32
    }
}

/// For Rust tests only (not in the app's simulator API).
impl SimGitHub {
    /// From now on, every pack stops halfway until [`Self::release_packs`]
    /// (or 30 seconds): a fetch stalled mid-transfer.
    pub fn stall_packs(&self) {
        lock(&self.shared.stall.state).armed = true;
    }

    pub fn stalled_packs(&self) -> u32 {
        lock(&self.shared.stall.state).waiting
    }

    /// Lets every stalled pack finish, and stalls no more.
    pub fn release_packs(&self) {
        lock(&self.shared.stall.state).armed = false;
        self.shared.stall.changed.notify_all();
    }
}

fn store_json(shared: &Shared, web: &str) -> serde_json::Value {
    json!({
        "id": STORE_ID,
        "name": shared.name,
        "full_name": format!("{}/{}", shared.owner, shared.name),
        "owner": { "login": shared.owner },
        "private": true,
        "default_branch": "main",
        "ssh_url": shared.ssh_address,
        "html_url": format!("{web}/{}/{}", shared.owner, shared.name),
        "pushed_at": "2026-11-20T09:12:00Z",
    })
}

fn other_json(shared: &Shared, index: u32) -> serde_json::Value {
    json!({
        "id": 10_000 + u64::from(index),
        "name": format!("other-{index:04}"),
        "full_name": format!("{}/other-{index:04}", shared.owner),
        "owner": { "login": shared.owner },
        "private": index.is_multiple_of(2),
        "default_branch": "main",
        "ssh_url": format!("git@github.com:{}/other-{index:04}.git", shared.owner),
        "pushed_at": format!("2025-01-01T00:{:02}:{:02}Z", (index / 60) % 60, index % 60),
    })
}

fn bearer(request: &SimHttpRequest) -> Option<String> {
    request.header("authorization")?.strip_prefix("Bearer ").map(str::to_owned)
}

fn basic_token(request: &SimHttpRequest) -> Option<String> {
    let encoded = request.header("authorization")?.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix("x-access-token:").map(str::to_owned)
}

fn issue(shared: &Shared, state: &mut State) -> serde_json::Value {
    state.issued += 1;
    let access = format!("ghu_sim{}{}", state.issued, random_hex(16));
    let web = lock(&shared.web).clone();
    let port = web.rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(0);
    lock(&ISSUED).push((port, access.clone()));
    if state.expiring {
        let refresh = format!("ghr_sim{}{}", state.issued, random_hex(16));
        lock(&ISSUED).push((port, refresh.clone()));
        state.access.insert(access.clone(), Some(refresh.clone()));
        state.refresh.push(refresh.clone());
        json!({
            "access_token": access,
            "expires_in": 28_800,
            "refresh_token": refresh,
            "refresh_token_expires_in": 15_897_600,
            "scope": "",
            "token_type": "bearer",
        })
    } else {
        state.access.insert(access.clone(), None);
        json!({ "access_token": access, "scope": "", "token_type": "bearer" })
    }
}

fn oauth_error(error: &str) -> Reply {
    Reply::json(200, json!({ "error": error, "error_description": "simulated" }))
}

fn handle(shared: &Shared, request: &SimHttpRequest) -> Reply {
    let path = request.path.as_str();
    let repo_prefix = format!("/{}/{}.git/", shared.owner, shared.name);
    match (request.method.as_str(), path) {
        ("POST", "/login/device/code") => device_code(shared, request),
        ("POST", "/login/oauth/access_token") => access_token(shared, request),
        ("POST", "/credentials/revoke") => revoke(shared, request),
        ("GET", "/user/installations") => installations(shared, request),
        ("GET", p) if p.starts_with("/user/installations/") => repositories(shared, request),
        (_, p) if p.starts_with(&repo_prefix) => git(shared, request, &p[repo_prefix.len()..]),
        _ => Reply::status(404),
    }
}

fn device_code(shared: &Shared, request: &SimHttpRequest) -> Reply {
    let form = request.form();
    if form.get("client_id") != Some(&*lock(&shared.client_id)) {
        return Reply::status(404);
    }
    let mut state = lock(&shared.state);
    if state.outcome == SimDeviceOutcome::Disabled {
        return oauth_error("device_flow_disabled");
    }
    let device_code = random_hex(20);
    state.pending = Some(Pending { device_code: device_code.clone(), polls: 0, decided: None });
    let web = lock(&shared.web).clone();
    Reply::json(
        200,
        json!({
            "device_code": device_code,
            "user_code": SIM_USER_CODE,
            "verification_uri": format!("{web}/login/device"),
            "expires_in": state.expires_in,
            "interval": state.interval,
        }),
    )
}

fn access_token(shared: &Shared, request: &SimHttpRequest) -> Reply {
    let form = request.form();
    if form.get("client_id") != Some(&*lock(&shared.client_id)) {
        return oauth_error("incorrect_client_credentials");
    }
    let mut state = lock(&shared.state);
    match form.get("grant_type").map(String::as_str) {
        Some("urn:ietf:params:oauth:grant-type:device_code") => {
            let outcome = state.outcome;
            let Some(pending) = state.pending.as_mut() else {
                return oauth_error("incorrect_device_code");
            };
            if form.get("device_code") != Some(&pending.device_code) {
                return oauth_error("incorrect_device_code");
            }
            pending.polls += 1;
            let polls = pending.polls;
            let decided = pending.decided;
            let answer = match outcome {
                SimDeviceOutcome::AuthoriseAfter { pending } if polls > pending => Some(true),
                SimDeviceOutcome::SlowDownThenAuthorise { .. } if polls == 1 => {
                    let interval = state.interval + 5;
                    state.interval = interval;
                    return Reply::json(200, json!({ "error": "slow_down", "interval": interval }));
                }
                SimDeviceOutcome::SlowDownThenAuthorise { pending } if polls > pending + 1 => Some(true),
                SimDeviceOutcome::DenyAfter { pending } if polls > pending => Some(false),
                SimDeviceOutcome::ExpireAfter { pending } if polls > pending => {
                    state.pending = None;
                    return oauth_error("expired_token");
                }
                SimDeviceOutcome::WaitForPerson => decided,
                _ => None,
            };
            match answer {
                None => oauth_error("authorization_pending"),
                Some(false) => {
                    state.pending = None;
                    oauth_error("access_denied")
                }
                Some(true) => {
                    state.pending = None;
                    Reply::json(200, issue(shared, &mut state))
                }
            }
        }
        Some("refresh_token") => {
            let Some(refresh) = form.get("refresh_token") else {
                return oauth_error("bad_refresh_token");
            };
            let Some(at) = state.refresh.iter().position(|r| r == refresh) else {
                return oauth_error("bad_refresh_token");
            };
            // Rotation: the refresh token and its access token end now.
            state.refresh.remove(at);
            state.access.retain(|_, r| r.as_deref() != Some(refresh.as_str()));
            Reply::json(200, issue(shared, &mut state))
        }
        _ => oauth_error("unsupported_grant_type"),
    }
}

fn revoke(shared: &Shared, request: &SimHttpRequest) -> Reply {
    if request.header("authorization").is_some() {
        return Reply::status(403);
    }
    let Ok(body) = serde_json::from_slice::<serde_json::Value>(&request.body) else {
        return Reply::status(422);
    };
    let mut state = lock(&shared.state);
    for credential in body["credentials"].as_array().into_iter().flatten().filter_map(|c| c.as_str()) {
        state.access.remove(credential);
        state.refresh.retain(|r| r != credential);
        state.revoked.push(credential.to_owned());
    }
    Reply::status(202)
}

fn authorised(shared: &Shared, token: Option<String>) -> bool {
    token.is_some_and(|t| lock(&shared.state).access.contains_key(&t))
}

fn installations(shared: &Shared, request: &SimHttpRequest) -> Reply {
    if !authorised(shared, bearer(request)) {
        return Reply::status(401);
    }
    if !lock(&shared.state).installed {
        return Reply::json(200, json!({ "total_count": 0, "installations": [] }));
    }
    Reply::json(
        200,
        json!({
            "total_count": 1,
            "installations": [{
                "id": INSTALLATION_ID,
                "app_slug": crate::github::APP_SLUG,
                "account": { "login": shared.owner },
            }],
        }),
    )
}

fn repositories(shared: &Shared, request: &SimHttpRequest) -> Reply {
    if !authorised(shared, bearer(request)) {
        return Reply::status(401);
    }
    if request.path != format!("/user/installations/{INSTALLATION_ID}/repositories") {
        return Reply::status(404);
    }
    let state = lock(&shared.state);
    if !state.installed {
        return Reply::status(404);
    }
    let web = lock(&shared.web).clone();
    let mut all = vec![store_json(shared, &web)];
    all.extend((0..state.extra_repositories).map(|i| other_json(shared, i)));
    let per_page: usize = request.query_value("per_page").and_then(|v| v.parse().ok()).unwrap_or(30).clamp(1, 100);
    let page: usize = request.query_value("page").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    let slice: Vec<_> = all.iter().skip((page - 1) * per_page).take(per_page).cloned().collect();
    Reply::json(200, json!({ "total_count": all.len(), "repository_selection": "selected", "repositories": slice }))
}

fn git(shared: &Shared, request: &SimHttpRequest, rest: &str) -> Reply {
    let (redirect, drop_mid_pack) = {
        let state = lock(&shared.state);
        (state.redirect_git.clone(), state.drop_mid_pack)
    };
    if let Some(to) = redirect {
        let mut reply = Reply::status(302);
        reply.headers.push(("Location".into(), format!("{to}{}{}", request.path, if request.query.is_empty() { String::new() } else { format!("?{}", request.query) })));
        return reply;
    }
    if !authorised(shared, basic_token(request)) {
        let mut reply = Reply::status(401);
        reply.headers.push(("WWW-Authenticate".into(), "Basic realm=\"GitHub\"".into()));
        return reply;
    }
    match (request.method.as_str(), rest) {
        ("GET", "info/refs") if request.query_value("service").as_deref() == Some("git-upload-pack") => {
            let Ok((refs, head)) = branches(&shared.bare) else {
                return Reply::status(500);
            };
            let mut body = pkt_line("# service=git-upload-pack\n");
            body.extend_from_slice(FLUSH);
            // `multi_ack_detailed`, as GitHub: without it libgit2's
            // stateless negotiation, past 20 haves, ends with a request
            // that has no wants.
            body.extend(advertisement(&refs, head.as_deref(), &format!("multi_ack_detailed ofs-delta {AGENT}")));
            Reply::bytes(200, "application/x-git-upload-pack-advertisement", body)
        }
        ("POST", "git-upload-pack") => match upload_pack(shared, &request.body) {
            Ok(body) => {
                let mut reply = Reply::bytes(200, "application/x-git-upload-pack-result", body);
                if drop_mid_pack {
                    reply.cut_at = Some(reply.body.len() / 2);
                }
                if lock(&shared.stall.state).armed {
                    reply.stall = Some(shared.stall.clone());
                }
                reply
            }
            Err(_) => Reply::status(400),
        },
        // No push over HTTPS here either.
        _ => Reply::status(403),
    }
}

fn reachable(bare: &std::path::Path, tip: Oid, id: Oid) -> bool {
    git2::Repository::open_bare(bare).and_then(|repo| repo.graph_descendant_of(tip, id)).unwrap_or(false)
}

/// One stateless upload-pack round: wants, haves, then `done` or a flush.
fn upload_pack(shared: &Shared, body: &[u8]) -> Result<Vec<u8>, String> {
    let mut rest = body;
    let mut lines: Vec<Option<String>> = Vec::new();
    while !rest.is_empty() {
        let (pkt, used) = split_pkt(rest)?.ok_or("short pkt-line")?;
        lines.push(match pkt {
            Pkt::Flush => None,
            Pkt::Line(line) => Some(String::from_utf8_lossy(&line).trim_end().to_owned()),
        });
        rest = &rest[used..];
    }
    let (refs, _) = branches(&shared.bare)?;
    let mut wants: Vec<Oid> = Vec::new();
    let mut haves: Vec<Oid> = Vec::new();
    let mut done = false;
    let mut in_wants = true;
    for line in lines {
        match line {
            None => in_wants = false,
            Some(line) if in_wants => {
                let id = line.strip_prefix("want ").ok_or("expected want")?.split(' ').next().unwrap_or_default();
                let id = parse_oid(id)?;
                // Each round is a new request, and the branch may have moved
                // since the advertisement: a commit reachable from a branch
                // is served, as GitHub does.
                if !refs.iter().any(|(tip, _)| *tip == id || reachable(&shared.bare, *tip, id)) {
                    return Err("not our ref".into());
                }
                wants.push(id);
            }
            Some(line) if line == "done" => done = true,
            Some(line) => haves.push(parse_oid(line.strip_prefix("have ").ok_or("expected have")?)?),
        }
    }
    let common = known_commits(&shared.bare, &haves)?;
    // `multi_ack_detailed`: a round that is not the last names each have
    // in common, then NAK; the last ends with one plain ACK (or NAK) and
    // the pack.
    if !done {
        let mut out: Vec<u8> = common.iter().flat_map(|id| pkt_line(&format!("ACK {id} common\n"))).collect();
        out.extend(pkt_line("NAK\n"));
        return Ok(out);
    }
    let mut out = match common.first() {
        Some(id) => pkt_line(&format!("ACK {id}\n")),
        None => pkt_line("NAK\n"),
    };
    out.extend(build_pack(&shared.bare, &wants, &common)?);
    Ok(out)
}

/// Another host on another port that answers everything with 200 and
/// records what it was sent: where a hostile redirect points.
pub struct SimBystander {
    record: Arc<Mutex<Vec<SimHttpRequest>>>,
    server: Server,
}

impl SimBystander {
    pub fn requests(&self) -> Vec<SimHttpRequest> {
        lock(&self.record).clone()
    }

    pub fn start() -> Result<Arc<Self>, PawError> {
        let record = Arc::new(Mutex::new(Vec::new()));
        let server = Server::start(
            record.clone(),
            Arc::new(|_| Reply::bytes(200, "application/x-git-upload-pack-advertisement", b"001e# service=git-upload-pack\n0000".to_vec())),
        )?;
        Ok(Arc::new(Self { record, server }))
    }

    pub fn url(&self) -> String {
        self.server.url()
    }

    pub fn request_count(&self) -> u32 {
        lock(&self.record).len() as u32
    }
}

/// The keychain in memory. It can be told to fail the next store, as if
/// the app were killed before the keychain write.
#[derive(Default, uniffi::Object)]
pub struct SimTokenVault {
    items: Mutex<HashMap<String, Vec<u8>>>,
    fail_next_store: AtomicBool,
    stores: AtomicU64,
}

impl SimTokenVault {
    /// A copy of every item (for scanning in tests).
    pub fn items(&self) -> HashMap<String, Vec<u8>> {
        lock(&self.items).clone()
    }
}

#[uniffi::export]
impl SimTokenVault {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn as_vault(self: Arc<Self>) -> Arc<dyn TokenVault> {
        self
    }
}

impl SimTokenVault {
    /// The next `store` fails and keeps nothing.
    pub fn fail_next_store(&self) {
        self.fail_next_store.store(true, Ordering::SeqCst);
    }

    pub fn store_count(&self) -> u64 {
        self.stores.load(Ordering::SeqCst)
    }

    pub fn is_empty(&self) -> bool {
        lock(&self.items).is_empty()
    }
}

impl TokenVault for SimTokenVault {
    fn store(&self, name: String, secret: Vec<u8>) -> Result<(), VaultError> {
        if self.fail_next_store.swap(false, Ordering::SeqCst) {
            return Err(VaultError::Failed { message: "simulated: the app was killed".into() });
        }
        self.stores.fetch_add(1, Ordering::SeqCst);
        lock(&self.items).insert(name, secret);
        Ok(())
    }

    fn load(&self, name: String) -> Result<Option<Vec<u8>>, VaultError> {
        Ok(lock(&self.items).get(&name).cloned())
    }

    fn remove(&self, name: String) -> Result<(), VaultError> {
        lock(&self.items).remove(&name);
        Ok(())
    }
}

/// A clock for the device flow that never sleeps: waiting moves it on, and
/// every wait is recorded.
#[derive(Default)]
pub struct SimPollClock {
    now: Mutex<Duration>,
    waits: Mutex<Vec<Duration>>,
}

impl SimPollClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The times polls were waited for, in order.
    pub fn waits(&self) -> Vec<Duration> {
        lock(&self.waits).clone()
    }

    pub fn advance(&self, by: Duration) {
        *lock(&self.now) += by;
    }
}

impl PollClock for SimPollClock {
    fn now(&self) -> Duration {
        *lock(&self.now)
    }

    fn wait_until(&self, at: Duration, cancel: &Cancel) -> bool {
        lock(&self.waits).push(at);
        if cancel.is_set() {
            return false;
        }
        let mut now = lock(&self.now);
        if *now < at {
            *now = at;
        }
        true
    }
}

//! First-run setup (design 2.4) as a state machine, with no UI in it.
//!
//! ```text
//! ReadKey ─► Address ─► CheckHost ─┬─────────────────► NeedPin ─► (Cloning) ─► TestDecrypt ─► Done
//!                                  └─► ConfirmHost ─┘
//! ```
//!
//! Each step is one call. A failure is a [`SetupError`] and leaves the flow
//! at the step that failed, so the same call can be tried again. The step
//! reached is saved in a small file in the state directory after every
//! change, with nothing secret in it. If the app is killed,
//! [`SetupFlow::open`] picks up where it stopped; a download that was
//! interrupted is removed and starts again.
//!
//! What it keeps where in the state directory is in [`crate::layout`].
//!
//! The steps that need the key run as key tasks on the key session, which
//! share the PIN typed once (see `ffi::sync`); they go through the
//! `begin_*` and `finish_*` steps here.

pub mod outcome;

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::key::enrolment::save_enrolment;
use crate::key::{CardInfo, KeyIdentity, KeyTouch, PinKdfStatus};
use crate::recipients::Access;
use crate::ssh::SshPublicKey;
use crate::store::PassStore;
use crate::sync::address::{AddressNotice, AddressProblem, StoreAddress};
use crate::sync::hosts::{self, KnownHosts};
use crate::sync::{self, Cloned, Connection, PresentedHostKey, RefusedPath};
use crate::write::{Keyring, LineStatus, WriteError};
use crate::PawError;
use outcome::{RemoteMessage, RemoteReason};
use crate::key::touch_clock::{TouchClock, TouchExpectation};

/// The key as setup shows it on "Your key". Public data only.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeySummary {
    /// Manufacturer and serial, e.g. "0006:12345678".
    pub ident: String,
    /// The SSH public key line to add to the git host.
    pub ssh_public_key: String,
    pub decryption_fingerprint: String,
    pub authentication_touch: KeyTouch,
    pub decryption_touch: KeyTouch,
}

/// Where setup is, and what the screen for it needs.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SetupState {
    /// Plug in the key; then `read_key_from_session`.
    ReadKey,
    /// Ask for the store's address; then `set_address`.
    Address { key: KeySummary },
    /// The address is set; `check_host` connects, checks the host's key and
    /// asks whether the host accepts this key. No PIN, no touch.
    CheckHost { address: String, notice: Option<AddressNotice> },
    /// The host is not known yet. Show the fingerprint; `confirm_host` with
    /// it pins the key and goes on.
    ConfirmHost { host: String, algorithm: String, fingerprint: String },
    /// Ask for the PIN; `download_with_key` clones the store with one login.
    NeedPin { address: String, touch: TouchExpectation },
    /// Cloned; `test_decrypt_with_key` opens one entry to prove the key works.
    TestDecrypt { entries: u32, touch: TouchExpectation },
    Done { opened: String, entries: u32 },
}

/// Why a setup step did not complete. Each is something the person can act
/// on, so each has its own case.
#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum SetupError {
    #[error("not an address paw can use: {problem:?}")]
    BadAddress { problem: AddressProblem },
    #[error("can't reach {host}: {detail}")]
    HostUnreachable { host: String, detail: String },
    /// A pinned host presented another key. Nothing was sent.
    #[error("{host}'s identity changed: expected {}, it presented {presented}", .expected.join(" or "))]
    HostKeyChanged { host: String, expected: Vec<String>, presented: String },
    /// `confirm_host` was given a fingerprint other than the one shown.
    #[error("that is not the fingerprint {host} presented")]
    HostKeyNotConfirmed { host: String },
    /// See [`PawError::HostKeyBundled`].
    #[error("{host}'s keys come with paw and can't be forgotten")]
    HostKeyBundled { host: String },
    /// The host does not know this key. Add `public_key` to the account.
    #[error("{host} doesn't know this key yet")]
    KeyNotAccepted { host: String, public_key: String },
    #[error("that store is empty")]
    EmptyRepository,
    #[error("that repository isn't a pass store: it has no .gpg-id file at the top")]
    NotAPassStore,
    #[error("cloned, but this key can't open any entry in the store")]
    KeyCannotOpenStore,
    #[error("paw downloaded your store but couldn't open {entry}: {detail}")]
    CannotOpenEntry { entry: String, detail: String },
    #[error("this key cannot be used: {reason}")]
    UnusableKey { reason: UnusableKeyReason },
    #[error("{message}")]
    WrongStep { message: String },
    /// See [`PawError::StoreBusy`].
    #[error("{}", crate::lock::STORE_BUSY)]
    StoreBusy,
    #[error("{message}")]
    Failed { message: String },
}

/// Why a key cannot set up a store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Enum)]
pub enum UnusableKeyReason {
    /// Its PIN KDF is one paw cannot follow. No PIN was sent to it.
    /// `detail` says what the card asked for, when known.
    #[error("this key uses a PIN setting (KDF) that paw doesn't support yet. paw has not sent it the PIN, so no try was used")]
    PinKdfUnsupported { detail: Option<String> },
    #[error("it has no authentication key, so it cannot log in to a git host")]
    NoAuthenticationKey,
    #[error("it has no decryption key")]
    NoDecryptionKey,
    /// Another key than the one read at "Your key".
    #[error("this is not the key read at the start of setup; plug that one in")]
    NotTheKeyRead,
    /// The key read from the card is not the one its fingerprint names.
    #[error("the key read from the card does not match its fingerprint")]
    CardKeyMismatch,
    /// The SSH key kept at "Your key" can't be read back as ed25519.
    #[error("{detail}")]
    SshKeyUnreadable { detail: String },
}

fn failed(error: impl std::fmt::Display) -> SetupError {
    SetupError::Failed { message: error.to_string() }
}

impl From<PawError> for SetupError {
    fn from(error: PawError) -> Self {
        match error {
            PawError::StoreBusy => Self::StoreBusy,
            PawError::HostKeyBundled { host } => Self::HostKeyBundled { host },
            other => failed(other),
        }
    }
}

/// Offered when a host could not be reached on its usual port: "Port 22
/// seems blocked. Try GitHub's port 443?"
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PortFallback {
    /// The host and port that could not be reached, e.g. github.com, 22.
    pub from_host: String,
    pub from_port: u16,
    /// Where to try instead, e.g. ssh.github.com, 443.
    pub host: String,
    pub port: u16,
    /// The store's address on that endpoint, as it would be saved:
    /// `ssh://git@ssh.github.com:443/you/store.git`.
    pub address: String,
}

/// Whether new entries can be saved once setup is done. Reading works
/// either way; saving needs a public key on this device for every
/// recipient `.gpg-id` names.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SetupWriteReadiness {
    /// Entries can be saved at the top of the store with no further step.
    pub can_save: bool,
    /// Lines of the top-level `.gpg-id` paw has no key for (or cannot tell
    /// which pinned key they mean). Add those keys, or bind the lines, to
    /// save. Empty when `can_save`.
    pub missing_recipients: Vec<String>,
    /// Other `.gpg-id` files (e.g. `archive/.gpg-id`) whose folders cannot
    /// be saved in yet, for the same reason.
    pub folders_needing_keys: Vec<String>,
}

/// A wrong step or a damaged store, as setup words it.
fn write_failed(error: WriteError) -> SetupError {
    failed(format!("could not record the store's keys: {error}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    ReadKey,
    Address,
    CheckHost,
    NeedPin,
    Cloning,
    TestDecrypt,
    Done,
}

impl Step {
    const ALL: [(Step, &'static str); 7] = [
        (Step::ReadKey, "read-key"),
        (Step::Address, "address"),
        (Step::CheckHost, "check-host"),
        (Step::NeedPin, "need-pin"),
        (Step::Cloning, "cloning"),
        (Step::TestDecrypt, "test-decrypt"),
        (Step::Done, "done"),
    ];

    fn name(self) -> &'static str {
        Self::ALL.iter().find(|(s, _)| *s == self).map(|(_, n)| *n).unwrap_or("read-key")
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, n)| *n == name).map(|(s, _)| *s)
    }
}

fn touch_name(touch: KeyTouch) -> &'static str {
    match touch {
        KeyTouch::Off => "off",
        KeyTouch::EachUse => "each-use",
        KeyTouch::Cached => "cached",
        KeyTouch::Unknown => "unknown",
    }
}

fn touch_from(name: &str) -> KeyTouch {
    match name {
        "off" => KeyTouch::Off,
        "each-use" => KeyTouch::EachUse,
        "cached" => KeyTouch::Cached,
        _ => KeyTouch::Unknown,
    }
}

/// What is saved between launches. Nothing in it is secret.
#[derive(Debug, Clone)]
struct Saved {
    step: Step,
    key: Option<KeySummary>,
    address: Option<String>,
    notice: Option<AddressNotice>,
    opened: Option<String>,
    /// Why a download sent setup back to the address: an empty repository
    /// or one that is not a pass store. Saved, so the address screen can
    /// still say so after a relaunch.
    problem: Option<RemoteReason>,
}

impl Saved {
    fn fresh() -> Self {
        Self { step: Step::ReadKey, key: None, address: None, notice: None, opened: None, problem: None }
    }

    fn to_text(&self) -> String {
        let mut lines = vec![format!("step={}", self.step.name())];
        if let Some(key) = &self.key {
            lines.push(format!("ident={}", key.ident));
            lines.push(format!("ssh_public_key={}", key.ssh_public_key));
            lines.push(format!("decryption_fingerprint={}", key.decryption_fingerprint));
            lines.push(format!("authentication_touch={}", touch_name(key.authentication_touch)));
            lines.push(format!("decryption_touch={}", touch_name(key.decryption_touch)));
        }
        let notice = self.notice.as_ref().map(notice_line);
        for (name, value) in [("address", &self.address), ("notice", &notice), ("opened", &self.opened)] {
            if let Some(value) = value {
                lines.push(format!("{name}={value}"));
            }
        }
        if let Some(problem) = self.problem {
            lines.push(format!("problem={}", problem.name()));
        }
        lines.iter().map(|line| format!("{}\n", line.replace(['\n', '\r'], " "))).collect()
    }

    /// Anything unreadable means starting setup again, which is always safe.
    fn from_text(text: &str) -> Self {
        let value = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
                .map(str::to_owned)
        };
        let Some(step) = value("step").as_deref().and_then(Step::from_name) else {
            return Self::fresh();
        };
        let key = match (value("ident"), value("ssh_public_key"), value("decryption_fingerprint")) {
            (Some(ident), Some(ssh_public_key), Some(decryption_fingerprint)) => Some(KeySummary {
                ident,
                ssh_public_key,
                decryption_fingerprint,
                authentication_touch: touch_from(&value("authentication_touch").unwrap_or_default()),
                decryption_touch: touch_from(&value("decryption_touch").unwrap_or_default()),
            }),
            _ => None,
        };
        let saved = Self {
            step,
            key,
            address: value("address"),
            notice: value("notice").as_deref().and_then(notice_from),
            opened: value("opened"),
            problem: value("problem").as_deref().and_then(RemoteReason::from_name),
        };
        // Every step past the first needs the key, and past Address the address.
        let complete = match step {
            Step::ReadKey => true,
            Step::Address => saved.key.is_some(),
            Step::Done => saved.key.is_some() && saved.address.is_some() && saved.opened.is_some(),
            _ => saved.key.is_some() && saved.address.is_some(),
        };
        if complete { saved } else { Self::fresh() }
    }
}

/// A notice as the setup file keeps it: its kind, then its values, one space
/// apart (none of them can hold a space).
fn notice_line(notice: &AddressNotice) -> String {
    match notice {
        AddressNotice::GitHubShortForm { owner, repo, address } => format!("github-short-form {owner} {repo} {address}"),
        AddressNotice::GitHubWithoutUser { address } => format!("github-without-user {address}"),
        AddressNotice::GitHubWebAddress { address } => format!("github-web-address {address}"),
        AddressNotice::PortFallback { from_host, from_port, host, port } => {
            format!("port-fallback {from_host} {from_port} {host} {port}")
        }
    }
}

/// [`notice_line`] read back. A notice an earlier version saved in words
/// reads as none.
fn notice_from(line: &str) -> Option<AddressNotice> {
    let fields: Vec<&str> = line.split(' ').collect();
    let text = |value: &str| value.to_owned();
    Some(match fields.as_slice() {
        ["github-short-form", owner, repo, address] => {
            AddressNotice::GitHubShortForm { owner: text(owner), repo: text(repo), address: text(address) }
        }
        ["github-without-user", address] => AddressNotice::GitHubWithoutUser { address: text(address) },
        ["github-web-address", address] => AddressNotice::GitHubWebAddress { address: text(address) },
        ["port-fallback", from_host, from_port, host, port] => AddressNotice::PortFallback {
            from_host: text(from_host),
            from_port: from_port.parse().ok()?,
            host: text(host),
            port: port.parse().ok()?,
        },
        _ => return None,
    })
}

/// The setup state machine for one store, kept in `state_dir`.
pub struct SetupFlow {
    dir: PathBuf,
    saved: Saved,
    /// The key a host presented that is waiting for the person's confirmation.
    /// Not saved: after a restart the host is asked again.
    unconfirmed: Option<PresentedHostKey>,
    authentication: TouchClock,
    decryption: TouchClock,
    /// What the last download refused to check out, if it did. Not saved.
    refused: Vec<RefusedPath>,
    /// What the last host check or download found out about the remote.
    remote_message: Option<RemoteMessage>,
    /// The last failure made no connection at all (see
    /// [`Self::port_fallback`]).
    no_connection: bool,
    route: sync::Route,
}

fn remove_dir(path: &Path, _held: &crate::lock::Exclusive) -> Result<(), SetupError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(failed(format!("could not remove the unfinished download: {e}"))),
    }
}

impl SetupFlow {
    /// Opens the setup kept in `state_dir`, creating the directory if need
    /// be, and resumes where it stopped.
    pub fn open(state_dir: &Path) -> Result<Self, SetupError> {
        std::fs::create_dir_all(state_dir).map_err(failed)?;
        let saved = match std::fs::read_to_string(state_dir.join(crate::layout::SETUP)) {
            Ok(text) => Saved::from_text(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved::fresh(),
            Err(e) => return Err(failed(e)),
        };
        let touch = |f: fn(&KeySummary) -> KeyTouch| {
            TouchClock::new(saved.key.as_ref().map(f).unwrap_or(KeyTouch::Unknown))
        };
        let mut flow = Self {
            dir: state_dir.to_path_buf(),
            authentication: touch(|k| k.authentication_touch),
            decryption: touch(|k| k.decryption_touch),
            saved,
            unconfirmed: None,
            refused: Vec::new(),
            remote_message: None,
            no_connection: false,
            route: sync::Route::default(),
        };
        if let Some(problem) = flow.saved.problem.filter(|_| flow.saved.step == Step::Address) {
            flow.note_remote(problem, None);
        }
        let repo = flow.repo_dir();
        let cloned = repo.join(".git").is_dir() && repo.join(".gpg-id").is_file();
        match flow.saved.step {
            // Killed while downloading, or the download went missing.
            Step::Cloning => flow.restart_download()?,
            Step::TestDecrypt | Step::Done if !cloned => flow.restart_download()?,
            _ => {}
        }
        Ok(flow)
    }

    /// Where the store is cloned to.
    pub fn repo_dir(&self) -> PathBuf {
        crate::layout::repo_dir(&self.dir)
    }

    fn known_hosts(&self) -> KnownHosts {
        KnownHosts::of(&self.dir)
    }

    fn save(&self) -> Result<(), SetupError> {
        let path = self.dir.join(crate::layout::SETUP);
        let temporary = self.dir.join(format!("{}.tmp", crate::layout::SETUP));
        std::fs::write(&temporary, self.saved.to_text()).map_err(failed)?;
        std::fs::rename(&temporary, &path).map_err(failed)
    }

    fn go(&mut self, step: Step) -> Result<SetupState, SetupError> {
        self.saved.step = step;
        if step != Step::Address {
            self.saved.problem = None;
        }
        self.save()?;
        Ok(self.state())
    }

    /// The store's lock, held: for removing or replacing the repository,
    /// and for the keyring and enrolment writes.
    fn exclusive(&self) -> Result<crate::lock::Exclusive, SetupError> {
        crate::lock::StoreLock::for_state_dir(&self.dir).exclusive().map_err(SetupError::from)
    }

    fn restart_download(&mut self) -> Result<(), SetupError> {
        remove_dir(&self.repo_dir(), &self.exclusive()?)?;
        self.saved.opened = None;
        self.go(Step::NeedPin).map(drop)
    }

    fn entry_count(&self) -> u32 {
        PassStore::open(self.repo_dir().to_string_lossy().into_owned())
            .map(|store| store.entry_count())
            .unwrap_or(0)
    }

    /// Where setup is now.
    pub fn state(&self) -> SetupState {
        let address = self.saved.address.clone().unwrap_or_default();
        if let Some(key) = &self.unconfirmed {
            return SetupState::ConfirmHost {
                host: self.address().map(|a| a.host).unwrap_or_default(),
                algorithm: key.algorithm(),
                fingerprint: key.fingerprint(),
            };
        }
        match (self.saved.step, &self.saved.key) {
            (Step::ReadKey, _) | (_, None) => SetupState::ReadKey,
            (Step::Address, Some(key)) => SetupState::Address { key: key.clone() },
            (Step::CheckHost, _) => SetupState::CheckHost { address, notice: self.saved.notice.clone() },
            (Step::NeedPin | Step::Cloning, _) => {
                SetupState::NeedPin { address, touch: self.authentication.expectation() }
            }
            (Step::TestDecrypt, _) => SetupState::TestDecrypt {
                entries: self.entry_count(),
                touch: self.decryption.expectation(),
            },
            (Step::Done, _) => SetupState::Done {
                opened: self.saved.opened.clone().unwrap_or_default(),
                entries: self.entry_count(),
            },
        }
    }

    fn expect(&self, steps: &[Step], doing: &str) -> Result<(), SetupError> {
        if steps.contains(&self.saved.step) && (self.unconfirmed.is_none() || doing == "confirm the host") {
            Ok(())
        } else {
            Err(SetupError::WrongStep {
                message: format!("cannot {doing} now: setup is at {}", self.saved.step.name()),
            })
        }
    }

    fn address(&self) -> Result<StoreAddress, SetupError> {
        let text = self.saved.address.as_deref().unwrap_or_default();
        StoreAddress::parse(text)
            .map(|parsed| parsed.address)
            .map_err(|problem| SetupError::BadAddress { problem })
    }

    fn key(&self) -> Result<&KeySummary, SetupError> {
        self.saved
            .key
            .as_ref()
            .ok_or_else(|| SetupError::WrongStep { message: "the key has not been read".into() })
    }

    /// Goes one step back: the Back button.
    pub fn back(&mut self) -> Result<SetupState, SetupError> {
        self.remote_message = None;
        if self.unconfirmed.take().is_some() {
            return Ok(self.state());
        }
        match self.saved.step {
            Step::ReadKey | Step::Done => Ok(self.state()),
            Step::Address => self.go(Step::ReadKey),
            Step::CheckHost => self.go(Step::Address),
            Step::NeedPin | Step::Cloning => self.go(Step::CheckHost),
            Step::TestDecrypt => {
                self.restart_download()?;
                Ok(self.state())
            }
        }
    }

    /// The key as its session read it when it was plugged in: no card
    /// exchange at all. `pin_kdf` is the session's; a key whose PIN KDF
    /// paw cannot follow is refused here (`UnusableKey`).
    pub fn read_key_from(
        &mut self,
        info: &CardInfo,
        ssh: Option<&SshPublicKey>,
        pin_kdf: PinKdfStatus,
    ) -> Result<SetupState, SetupError> {
        self.expect(&[Step::ReadKey, Step::Address], "read the key")?;
        if !pin_kdf.supported {
            return Err(SetupError::UnusableKey { reason: UnusableKeyReason::PinKdfUnsupported { detail: pin_kdf.detail } });
        }
        // A key with neither is named by its missing decryption key.
        if info.decryption_fingerprint.is_none() {
            return Err(SetupError::UnusableKey { reason: UnusableKeyReason::NoDecryptionKey });
        }
        let ssh = ssh.ok_or(SetupError::UnusableKey { reason: UnusableKeyReason::NoAuthenticationKey })?;
        self.use_key(info.clone(), ssh)
    }

    fn use_key(&mut self, info: CardInfo, ssh: &SshPublicKey) -> Result<SetupState, SetupError> {
        let Some(decryption_fingerprint) = info.decryption_fingerprint else {
            return Err(SetupError::UnusableKey { reason: UnusableKeyReason::NoDecryptionKey });
        };
        let key = KeySummary {
            ident: info.ident,
            ssh_public_key: ssh.authorized_key(),
            decryption_fingerprint,
            authentication_touch: info.authentication_touch,
            decryption_touch: info.decryption_touch,
        };
        self.authentication = TouchClock::new(key.authentication_touch);
        self.decryption = TouchClock::new(key.decryption_touch);
        self.saved.key = Some(key);
        self.go(Step::Address)
    }

    /// Sets the store's address from what the person typed.
    pub fn set_address(&mut self, text: &str) -> Result<SetupState, SetupError> {
        self.expect(&[Step::Address, Step::CheckHost], "set the address")?;
        let parsed = StoreAddress::parse(text).map_err(|problem| SetupError::BadAddress { problem })?;
        self.remote_message = None;
        self.saved.address = Some(parsed.address.display());
        self.saved.notice = parsed.notice;
        self.go(Step::CheckHost)
    }

    /// Connects to the host. A host with a pinned key (github.com ships
    /// with paw, others were confirmed before) has it checked, then is asked
    /// whether it accepts this key, without a signature. An unknown host
    /// leads to [`SetupState::ConfirmHost`] instead.
    pub fn check_host(&mut self) -> Result<SetupState, SetupError> {
        self.expect(&[Step::CheckHost, Step::NeedPin], "check the host")?;
        let address = self.address()?;
        let host = address.connect_host().to_owned();
        self.remote_message = None;
        match self.known_hosts().pin(&host, address.port).map_err(SetupError::from)? {
            Some(_) => self.ask_host(),
            None => {
                let scanned = sync::scan_host_key(&host, address.port, &self.route);
                let key = scanned.map_err(|e| match e {
                    PawError::Network(detail) => {
                        self.note_remote(RemoteReason::Unreachable, None);
                        self.no_connection = sync::made_no_connection(&detail);
                        SetupError::HostUnreachable { host: host.clone(), detail }
                    }
                    other => failed(other),
                })?;
                self.unconfirmed = Some(key);
                Ok(self.state())
            }
        }
    }

    /// The person compared the fingerprint and accepts the host's key.
    pub fn confirm_host(&mut self, fingerprint: &str) -> Result<SetupState, SetupError> {
        self.expect(&[Step::CheckHost, Step::NeedPin], "confirm the host")?;
        let address = self.address()?;
        let Some(key) = self.unconfirmed.take() else {
            return Err(SetupError::WrongStep { message: "no host key is waiting to be confirmed".into() });
        };
        let trusted = self.known_hosts().trust(address.connect_host(), address.port, || Ok(key.clone()), fingerprint);
        match trusted {
            Ok(true) => self.ask_host(),
            Ok(false) => {
                self.unconfirmed = Some(key);
                Err(SetupError::HostKeyNotConfirmed { host: address.host })
            }
            Err(e) => Err(failed(e)),
        }
    }

    /// "I expected this change": forgets the key pinned for the store's
    /// host, so the next [`Self::check_host`] shows the key it presents now
    /// for confirmation, as on first use. For the changed-host-key screen,
    /// after it has shown both fingerprints; setup stays where it is.
    /// Refused for a host whose keys ship with paw ([`KnownHosts::forget`]).
    pub fn forget_host_key(&mut self) -> Result<SetupState, SetupError> {
        self.expect(&[Step::CheckHost, Step::NeedPin], "forget the host's key")?;
        let address = self.address()?;
        self.known_hosts().forget(address.connect_host(), address.port).map_err(SetupError::from)?;
        self.remote_message = None;
        self.go(Step::CheckHost)
    }

    fn remote(&self) -> Result<sync::Remote, SetupError> {
        let address = self.address()?;
        let remote = self.known_hosts().remote(&address).map_err(SetupError::from)?;
        let mut remote = remote.ok_or_else(|| SetupError::WrongStep { message: "the host has not been checked".into() })?;
        remote.route = self.route.clone();
        Ok(remote)
    }

    /// Connections go where `route` says (a simulation playing a host).
    #[cfg(feature = "sim")]
    pub(crate) fn set_route(&mut self, route: sync::Route) {
        self.route = route;
    }

    /// The setup error for a failed connection or git command, noting what
    /// it says about the remote for [`Self::last_remote_message`]. `said`
    /// is what the host printed, if anything.
    pub(crate) fn connection_error(&mut self, remote: &sync::Remote, error: PawError, said: Option<String>) -> SetupError {
        let no_connection = matches!(&error, PawError::Network(detail) if sync::made_no_connection(detail));
        let (reason, error) = match error {
            PawError::Network(detail) => {
                (RemoteReason::Unreachable, SetupError::HostUnreachable { host: remote.host.clone(), detail })
            }
            PawError::HostKeyMismatch { presented } => (
                RemoteReason::HostKeyChanged,
                SetupError::HostKeyChanged {
                    host: remote.host.clone(),
                    expected: remote.host_key.fingerprints(),
                    presented,
                },
            ),
            PawError::KeyRejected => (
                RemoteReason::KeyNotAccepted,
                SetupError::KeyNotAccepted {
                    host: remote.host.clone(),
                    public_key: self.saved.key.as_ref().map(|k| k.ssh_public_key.clone()).unwrap_or_default(),
                },
            ),
            // The login went through, and the host refused the repository.
            _ if said.as_deref().is_some_and(sync::says_repository_missing) => (
                RemoteReason::RepositoryNotFound,
                failed(format!(
                    "no store at that address: {} says {} doesn't exist, or this key can't see it",
                    remote.host, remote.path
                )),
            ),
            other => (RemoteReason::Other, SetupError::from(other)),
        };
        self.note_remote(reason, said);
        self.no_connection = no_connection;
        error
    }

    fn note_remote(&mut self, reason: RemoteReason, said: Option<String>) {
        let (host, repository) = match self.address() {
            Ok(address) => (address.connect_host().to_owned(), address.path.trim_start_matches('/').to_owned()),
            Err(_) => (String::new(), String::new()),
        };
        self.remote_message = Some(RemoteMessage { reason, host, repository, said });
        self.no_connection = false;
    }

    /// After a host check or download that could not connect at all (the
    /// TCP connection was refused or timed out, as on a network that blocks
    /// port 22): another way in to the same host, if it has one. For
    /// github.com that is `ssh.github.com` on port 443, with the same
    /// bundled keys. Only offered: [`Self::use_port_fallback`] switches to
    /// it. `None` after any other outcome, including a changed host key or
    /// a refused key.
    pub fn port_fallback(&self) -> Option<PortFallback> {
        let reached_nothing = self.no_connection
            && self.remote_message.as_ref().is_some_and(|m| m.reason == RemoteReason::Unreachable);
        if !reached_nothing || !matches!(self.saved.step, Step::CheckHost | Step::NeedPin) {
            return None;
        }
        let address = self.address().ok()?;
        let (host, port) = hosts::port_fallback(address.connect_host(), address.port)?;
        let to = StoreAddress {
            user: address.user.clone(),
            host: host.to_owned(),
            port,
            path: format!("/{}", address.path.trim_start_matches('/')),
        };
        Some(PortFallback { from_host: address.connect_host().to_owned(), from_port: address.port, host: host.to_owned(), port, address: to.display() })
    }

    /// Switches the store's address to [`Self::port_fallback`]'s, keeping
    /// the repository, and goes back to the host check. Refused when none
    /// is offered.
    pub fn use_port_fallback(&mut self) -> Result<SetupState, SetupError> {
        self.expect(&[Step::CheckHost, Step::NeedPin], "use another port")?;
        let Some(fallback) = self.port_fallback() else {
            return Err(SetupError::WrongStep { message: "no other way to reach the host is offered now".into() });
        };
        self.saved.notice = Some(AddressNotice::PortFallback {
            from_host: fallback.from_host,
            from_port: fallback.from_port,
            host: fallback.host,
            port: fallback.port,
        });
        self.saved.address = Some(fallback.address);
        self.remote_message = None;
        self.no_connection = false;
        self.go(Step::CheckHost)
    }

    /// What the last host check or download found out about the remote,
    /// if it stopped for the remote's sake: which screen to show for a
    /// `SetupError::Failed` in particular. `None` after a step that went
    /// through, and after a failure that was the key's or the store's.
    pub fn last_remote_message(&self) -> Option<RemoteMessage> {
        self.remote_message.clone()
    }

    /// Asks the pinned host whether it accepts the key. No signature.
    fn ask_host(&mut self) -> Result<SetupState, SetupError> {
        self.remote_message = None;
        let remote = self.remote()?;
        let key = ssh_key(self.key()?)?;
        match Connection::probe(&remote, &key) {
            Ok(true) => self.go(Step::NeedPin),
            Ok(false) => Err(self.connection_error(&remote, PawError::KeyRejected, None)),
            Err(e) => Err(self.connection_error(&remote, e, None)),
        }
    }

    /// The start of a download: the remote to clone and where to, with any
    /// earlier attempt removed. Setup is at "cloning" until
    /// [`Self::finish_download`] or [`Self::abandon_download`].
    pub(crate) fn begin_download(&mut self) -> Result<(sync::Remote, PathBuf), SetupError> {
        self.expect(&[Step::NeedPin], "download")?;
        let remote = self.remote()?;
        let repo = self.repo_dir();
        remove_dir(&repo, &self.exclusive()?)?;
        self.refused.clear();
        self.remote_message = None;
        self.go(Step::Cloning)?;
        Ok((remote, repo))
    }

    /// The download stopped (cancelled, or the login failed): back to the
    /// PIN step, with nothing left of it.
    pub(crate) fn abandon_download(&mut self) -> Result<(), SetupError> {
        if self.saved.step == Step::Cloning {
            self.restart_download()?;
        }
        Ok(())
    }

    /// The login signed: a cached touch holds for a while from now.
    pub(crate) fn note_login(&mut self) {
        self.authentication.used_at(Instant::now());
    }

    /// The end of a download: checks what arrived is a pass store and moves
    /// on to the test decrypt, or starts the download over. `said` is what
    /// the host printed on stderr, if anything.
    ///
    /// A repository with no commits, or with no `.gpg-id` at the top, is
    /// the wrong address rather than a failed download: setup goes back to
    /// the address, with the address kept and the reason in
    /// [`Self::last_remote_message`] (also after a relaunch).
    pub(crate) fn finish_download(
        &mut self,
        remote: &sync::Remote,
        cloned: Result<Cloned, PawError>,
        said: Option<String>,
    ) -> Result<SetupState, SetupError> {
        let repo = self.repo_dir();
        let outcome = match cloned {
            Err(e) => Err(self.connection_error(remote, e, said)),
            Ok(Cloned::Empty) => return self.wrong_repository(RemoteReason::EmptyRepository),
            // Nothing was checked out; the download is removed below.
            Ok(Cloned::Refused(paths)) => {
                let error = failed(sync::Refused(paths.clone()));
                self.refused = paths;
                Err(error)
            }
            Ok(Cloned::Store) => {
                let gpg_id = std::fs::symlink_metadata(repo.join(".gpg-id"));
                if gpg_id.map(|m| m.is_file()).unwrap_or(false) {
                    Ok(())
                } else {
                    return self.wrong_repository(RemoteReason::NotAPassStore);
                }
            }
        };
        if let Err(e) = outcome {
            self.restart_download()?;
            return Err(e);
        }
        let address = self.saved.address.clone().unwrap_or_default();
        {
            let _held = self.exclusive()?;
            std::fs::write(self.dir.join(crate::layout::REMOTE), format!("{address}\n")).map_err(failed)?;
        }
        self.go(Step::TestDecrypt)
    }

    /// The repository is not a store paw can use: back to the address.
    fn wrong_repository(&mut self, reason: RemoteReason) -> Result<SetupState, SetupError> {
        remove_dir(&self.repo_dir(), &self.exclusive()?)?;
        self.saved.opened = None;
        self.note_remote(reason, None);
        self.saved.problem = Some(reason);
        self.go(Step::Address)?;
        Err(match reason {
            RemoteReason::EmptyRepository => SetupError::EmptyRepository,
            _ => SetupError::NotAPassStore,
        })
    }

    /// The entry the test decrypt opens: the first the key surely opens,
    /// else the first it might (hidden recipients).
    pub(crate) fn pick_test_entry(&self) -> Result<String, SetupError> {
        self.expect(&[Step::TestDecrypt], "test the key")?;
        let fingerprint = self.key()?.decryption_fingerprint.clone();
        let store = PassStore::open(self.repo_dir().to_string_lossy().into_owned()).map_err(SetupError::from)?;

        let mut maybe = None;
        let mut sure = None;
        for entry in store.entries() {
            match store.access(entry.path.clone(), fingerprint.clone()) {
                Access::CanOpen => {
                    sure = Some(entry.path.clone());
                    break;
                }
                Access::MightOpen if maybe.is_none() => maybe = Some(entry.path.clone()),
                _ => {}
            }
        }
        sure.or(maybe).ok_or(SetupError::KeyCannotOpenStore)
    }

    /// The key opened `entry`: completes what saving needs and records the
    /// key, then setup is done.
    ///
    /// - The card's decryption key (`card_key`) is pinned, and bound to the
    ///   one unsatisfied line of the `.gpg-id` that governs `entry`, if the
    ///   entry's recipient packets name the card's key and no line of that
    ///   file already means it. If paw cannot tell which line is the card's,
    ///   nothing is bound and [`Self::write_readiness`] says what is missing.
    /// - Every `.gpg-id` is accepted as it is now.
    /// - The key's identity is recorded, so the PIN goes to no other card.
    pub(crate) fn finish_test_decrypt(
        &mut self,
        entry: String,
        card_key: &pgp::packet::PublicKey,
        identity: &KeyIdentity,
    ) -> Result<SetupState, SetupError> {
        self.expect(&[Step::TestDecrypt], "finish the key check")?;
        let summary = self.key()?;
        if identity.decryption_fingerprint.as_deref().map(str::to_ascii_uppercase)
            != Some(summary.decryption_fingerprint.to_ascii_uppercase())
        {
            return Err(SetupError::UnusableKey { reason: UnusableKeyReason::NotTheKeyRead });
        }
        // The key about to be trusted for every future save must be the very
        // key whose fingerprint the card reported, not merely one handed in
        // alongside it.
        if !crate::write::cert::fingerprint_of(card_key)
            .eq_ignore_ascii_case(&summary.decryption_fingerprint)
        {
            return Err(SetupError::UnusableKey { reason: UnusableKeyReason::CardKeyMismatch });
        }
        let repo = self.repo_dir();
        // The keyring and the enrolment change as one critical section.
        let held = self.exclusive()?;
        let mut keyring = Keyring::open(&self.dir).map_err(write_failed)?;
        let fingerprint = keyring.pin_card_key(card_key).map_err(write_failed)?;
        // Only the `.gpg-id` that governs the entry just opened, and only if
        // that entry names the card's key, says which line is the card's.
        let store = PassStore::open(repo.to_string_lossy().into_owned()).map_err(SetupError::from)?;
        let ciphertext = store.read_entry(&entry).map_err(SetupError::from)?;
        let bound = keyring.bind_card_for_opened_entry(&repo, &fingerprint, &entry, &ciphertext, crate::write::now_secs());
        match bound {
            // Several unknown lines: the person picks the card's later.
            Ok(_) | Err(WriteError::MissingRecipients { .. }) => {}
            Err(e) => return Err(write_failed(e)),
        }
        keyring.accept_all_recipients_at_setup(&repo).map_err(write_failed)?;
        save_enrolment(&self.dir, identity).map_err(SetupError::from)?;
        drop(held);
        self.decryption.used_at(Instant::now());
        self.saved.opened = Some(entry);
        self.go(Step::Done)
    }

    /// Whether new entries can be saved, once setup is done. Reads the
    /// pinned keys and the store; changes nothing.
    pub fn write_readiness(&self) -> Result<SetupWriteReadiness, SetupError> {
        self.expect(&[Step::Done], "say whether entries can be saved")?;
        let repo = self.repo_dir();
        let keyring = Keyring::open(&self.dir).map_err(write_failed)?;
        let now = crate::write::now_secs();
        let top = keyring.folder_recipients(&repo, "", now).map_err(write_failed)?;
        let missing_recipients = if top.writable {
            Vec::new()
        } else {
            top.lines
                .iter()
                .filter(|line| !matches!(line.status, LineStatus::Ready { .. }))
                .map(|line| line.line.clone())
                .collect()
        };
        let mut folders_needing_keys = Vec::new();
        for gpg_id in crate::write::keyring::gpg_id_paths(&repo).map_err(write_failed)? {
            let folder = gpg_id.strip_suffix(".gpg-id").unwrap_or_default().trim_end_matches('/');
            if folder.is_empty() {
                continue;
            }
            let recipients = keyring.folder_recipients(&repo, folder, now).map_err(write_failed)?;
            if !recipients.writable {
                folders_needing_keys.push(gpg_id);
            }
        }
        Ok(SetupWriteReadiness { can_save: top.writable, missing_recipients, folders_needing_keys })
    }

    /// What the last download refused to check out: the paths and why.
    /// Empty unless the last download failed with "the remote contains
    /// something paw will not check out".
    pub fn refused_remote(&self) -> Vec<RefusedPath> {
        self.refused.clone()
    }

    /// The key setup read at the start, as an identity to hold a session to.
    pub(crate) fn expected_key(&self) -> Option<KeyIdentity> {
        self.saved.key.as_ref().map(|key| KeyIdentity {
            ident: key.ident.clone(),
            decryption_fingerprint: Some(key.decryption_fingerprint.clone()),
            authentication_fingerprint: None,
        })
    }
}

fn ssh_key(key: &KeySummary) -> Result<SshPublicKey, SetupError> {
    use base64::Engine;
    let encoded = key.ssh_public_key.split(' ').nth(1).unwrap_or_default();
    let blob = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| unreadable_ssh_key(e.to_string()))?;
    // An ssh-ed25519 blob ends in the 32-byte key.
    let raw: [u8; 32] = blob
        .get(blob.len().saturating_sub(32)..)
        .and_then(|tail| tail.try_into().ok())
        .ok_or_else(|| unreadable_ssh_key("not an ed25519 key".into()))?;
    let key = SshPublicKey::ed25519(&raw);
    if key.blob() != blob.as_slice() {
        return Err(unreadable_ssh_key("not an ed25519 key".into()));
    }
    Ok(key)
}

fn unreadable_ssh_key(detail: String) -> SetupError {
    SetupError::UnusableKey { reason: UnusableKeyReason::SshKeyUnreadable { detail } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_are_kept_and_one_saved_in_words_reads_as_none() {
        let notices = [
            AddressNotice::GitHubShortForm { owner: "you".into(), repo: "store.git".into(), address: "git@github.com:you/store.git".into() },
            AddressNotice::GitHubWithoutUser { address: "git@github.com:you/store.git".into() },
            AddressNotice::GitHubWebAddress { address: "git@github.com:you/store.git".into() },
            AddressNotice::PortFallback { from_host: "github.com".into(), from_port: 22, host: "ssh.github.com".into(), port: 443 },
        ];
        for notice in notices {
            assert_eq!(notice_from(&notice_line(&notice)), Some(notice));
        }
        let old = "Port 22 of github.com could not be reached, so paw will use ssh.github.com on port 443 instead.";
        let saved = Saved::from_text(&format!("step=check-host\nident=x\nssh_public_key=k\ndecryption_fingerprint=f\naddress=a\nnotice={old}\n"));
        assert_eq!(saved.step, Step::CheckHost);
        assert_eq!(saved.notice, None);
    }

    #[test]
    fn a_finished_setup_starts_with_the_line_layout_reads() {
        let done = Saved { step: Step::Done, ..Saved::fresh() };
        assert_eq!(done.to_text().lines().next(), Some(crate::layout::SETUP_DONE));
    }
}

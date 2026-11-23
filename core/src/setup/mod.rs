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
//! change, with nothing secret in it: the PIN is passed to the one call
//! that needs it and dropped when that call returns. If the app is killed,
//! [`SetupFlow::open`] picks up where it stopped; a download that was
//! interrupted is removed and starts again.
//!
//! The state directory holds:
//! - `setup` — the saved step (this module);
//! - `known_hosts` — host keys the person confirmed ([`crate::sync::hosts`]);
//! - `remote` — the store's address, once a clone succeeded;
//! - `enrolled-key` — the key's identity, once the test decrypt succeeded
//!   ([`crate::key::enrolment`]);
//! - `recipients` — the pinned keys and accepted `.gpg-id` files
//!   ([`crate::write::keyring`]), written by the same step;
//! - `repo/` — the store itself.
//!
//! The steps that need the key exist twice: with a card connection and the
//! PIN passed per call (`download`, `test_decrypt`), and as key tasks on
//! the key session, which share the PIN typed once (see `ffi::sync`). Both
//! go through the same `begin_*` and `finish_*` steps here, so they save
//! the same state and leave the same files.

pub mod address;
pub mod outcome;
pub mod touch;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use zeroize::Zeroizing;

use crate::key::KeyIdentity;
use crate::key::enrolment::save_enrolment;
use crate::recipients::Access;
use crate::ssh::{CardSigner, SshPublicKey, card_ssh_public_key};
use crate::store::PassStore;
use crate::sync::hosts::{KnownHosts, is_bundled, pin_for};
use crate::sync::{self, Cloned, Connection, PresentedHostKey, RefusedPath};
use crate::write::{Keyring, LineStatus, WriteError};
use crate::{ApduTransport, CardInfo, KeyTouch, PawError, card_info};
use address::StoreAddress;
use outcome::{RemoteMessage, RemoteReason, says_repository_missing};
use touch::{TouchClock, TouchExpectation};

const STATE_FILE: &str = "setup";
const KNOWN_HOSTS: &str = "known_hosts";
const REMOTE_FILE: &str = "remote";
const REPO_DIR: &str = "repo";

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
    /// Plug in the key; then `read_key`.
    ReadKey,
    /// Ask for the store's address; then `set_address`.
    Address { key: KeySummary },
    /// The address is set; `check_host` connects, checks the host's key and
    /// asks whether the host accepts this key. No PIN, no touch.
    CheckHost { address: String, notice: Option<String> },
    /// The host is not known yet. Show the fingerprint; `confirm_host` with
    /// it pins the key and goes on.
    ConfirmHost { host: String, algorithm: String, fingerprint: String },
    /// Ask for the PIN; `download` clones the store with one login.
    NeedPin { address: String, touch: TouchExpectation },
    /// Cloned; `test_decrypt` opens one entry to prove the key works.
    TestDecrypt { entries: u32, touch: TouchExpectation },
    Done { opened: String, entries: u32 },
}

/// Why a setup step did not complete. Each is something the person can act
/// on, so each has its own case.
#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum SetupError {
    #[error("{message}")]
    BadAddress { message: String },
    #[error("can't reach {host}: {detail}")]
    HostUnreachable { host: String, detail: String },
    /// A pinned host presented another key. Nothing was sent.
    #[error("{host}'s identity changed: expected {}, it presented {presented}", .expected.join(" or "))]
    HostKeyChanged { host: String, expected: Vec<String>, presented: String },
    /// `confirm_host` was given a fingerprint other than the one shown.
    #[error("that is not the fingerprint {host} presented")]
    HostKeyNotConfirmed { host: String },
    /// The host does not know this key. Add `public_key` to the account.
    #[error("{host} doesn't know this key yet")]
    KeyNotAccepted { host: String, public_key: String },
    #[error("wrong PIN, {tries_left} tries left")]
    WrongPin { tries_left: u8 },
    #[error("the PIN is blocked; unblock it with the admin PIN")]
    PinBlocked,
    #[error("the key was not touched in time")]
    TouchTimedOut,
    #[error("that store is empty")]
    EmptyRepository,
    #[error("that repository isn't a pass store: it has no .gpg-id file at the top")]
    NotAPassStore,
    #[error("cloned, but this key can't open any entry in the store")]
    KeyCannotOpenStore,
    #[error("paw downloaded your store but couldn't open {entry}: {detail}")]
    CannotOpenEntry { entry: String, detail: String },
    #[error("this key cannot be used: {message}")]
    UnusableKey { message: String },
    /// The progress callback asked to stop. The next `download` starts over.
    #[error("stopped")]
    Cancelled,
    #[error("{message}")]
    WrongStep { message: String },
    #[error("{message}")]
    Failed { message: String },
}

fn failed(error: impl std::fmt::Display) -> SetupError {
    SetupError::Failed { message: error.to_string() }
}

/// Another way in to a host whose usual port a network may block: from
/// (host, port) to (host, port), with the same bundled keys. GitHub's page
/// "Using SSH over the HTTPS port"
/// (<https://docs.github.com/en/authentication/troubleshooting-ssh/using-ssh-over-the-https-port>).
const PORT_FALLBACKS: [(&str, u16, &str, u16); 1] = [("github.com", 22, "ssh.github.com", 443)];

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

/// What setup tells the app while a step runs.
pub trait SetupEvents: Send + Sync {
    /// The key is about to wait for a touch, or may be.
    fn touch_needed(&self, expectation: TouchExpectation);
    /// Objects received and expected while cloning. Return `false` to stop.
    fn progress(&self, received: u32, total: u32) -> bool;
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
    notice: Option<String>,
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
        for (name, value) in [("address", &self.address), ("notice", &self.notice), ("opened", &self.opened)] {
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
            notice: value("notice"),
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
}

fn remove_dir(path: &Path) -> Result<(), SetupError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(failed(format!("could not remove the unfinished download: {e}"))),
    }
}

/// What a login or a key operation failing means to the person.
fn key_error(error: PawError, touch_asked: bool) -> SetupError {
    match error {
        PawError::WrongPin { tries_left } => SetupError::WrongPin { tries_left },
        PawError::PinBlocked => SetupError::PinBlocked,
        // The card refuses a signature or decryption it waited for and did
        // not get a touch for; with no touch asked, it is another failure.
        PawError::Sign(_) | PawError::Decrypt(_) | PawError::Card(_) if touch_asked => {
            SetupError::TouchTimedOut
        }
        other => failed(other),
    }
}

impl SetupFlow {
    /// Opens the setup kept in `state_dir`, creating the directory if need
    /// be, and resumes where it stopped.
    pub fn open(state_dir: &Path) -> Result<Self, SetupError> {
        std::fs::create_dir_all(state_dir).map_err(failed)?;
        let saved = match std::fs::read_to_string(state_dir.join(STATE_FILE)) {
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
        self.dir.join(REPO_DIR)
    }

    pub fn known_hosts(&self) -> KnownHosts {
        KnownHosts::new(&self.dir.join(KNOWN_HOSTS))
    }

    fn save(&self) -> Result<(), SetupError> {
        let path = self.dir.join(STATE_FILE);
        let temporary = self.dir.join(format!("{STATE_FILE}.tmp"));
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

    fn restart_download(&mut self) -> Result<(), SetupError> {
        remove_dir(&self.repo_dir())?;
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
            .map_err(|message| SetupError::BadAddress { message })
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

    /// Reads the plugged-in key: no PIN, no touch.
    /// A key whose PIN KDF paw cannot follow is refused here
    /// (`UnusableKey`), before any PIN is asked.
    pub fn read_key(&mut self, card: Arc<dyn ApduTransport>) -> Result<SetupState, SetupError> {
        self.expect(&[Step::ReadKey, Step::Address], "read the key")?;
        let info = card_info(card.clone()).map_err(failed)?;
        let kdf = crate::key::kdf::read_from(card.clone()).map_err(failed)?;
        if !kdf.status().supported {
            return Err(SetupError::UnusableKey { message: crate::key::kdf::UNSUPPORTED_AT_SETUP.into() });
        }
        let ssh = card_ssh_public_key(card).map_err(|e| SetupError::UnusableKey { message: e.to_string() })?;
        self.use_key(info, &ssh)
    }

    /// The key as its session read it when it was plugged in: no card
    /// exchange at all. `pin_kdf_supported` is the session's; a key whose
    /// PIN KDF paw cannot follow is refused here (`UnusableKey`).
    pub fn read_key_from(
        &mut self,
        info: &CardInfo,
        ssh: Option<&SshPublicKey>,
        pin_kdf_supported: bool,
    ) -> Result<SetupState, SetupError> {
        self.expect(&[Step::ReadKey, Step::Address], "read the key")?;
        if !pin_kdf_supported {
            return Err(SetupError::UnusableKey { message: crate::key::kdf::UNSUPPORTED_AT_SETUP.into() });
        }
        let ssh = ssh.ok_or_else(|| SetupError::UnusableKey {
            message: "it has no authentication key, so it cannot log in to a git host".into(),
        })?;
        self.use_key(info.clone(), ssh)
    }

    fn use_key(&mut self, info: CardInfo, ssh: &SshPublicKey) -> Result<SetupState, SetupError> {
        let Some(decryption_fingerprint) = info.decryption_fingerprint else {
            return Err(SetupError::UnusableKey { message: "it has no decryption key".into() });
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
        let parsed = StoreAddress::parse(text).map_err(|message| SetupError::BadAddress { message })?;
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
        match pin_for(&self.known_hosts(), &host, address.port).map_err(failed)? {
            Some(_) => self.ask_host(),
            None => {
                let scanned = sync::scan_host_key(&host, address.port);
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
        if key.fingerprint() != fingerprint.trim() {
            self.unconfirmed = Some(key);
            return Err(SetupError::HostKeyNotConfirmed { host: address.host });
        }
        let line = key.line().map_err(failed)?;
        self.known_hosts()
            .add(address.connect_host(), address.port, &line)
            .map_err(failed)?;
        self.ask_host()
    }

    /// "I expected this change": forgets the key pinned for the store's
    /// host, so the next [`Self::check_host`] shows the key it presents now
    /// for confirmation, as on first use. For the changed-host-key screen,
    /// after it has shown both fingerprints; setup stays where it is.
    ///
    /// A host whose keys ship with paw (github.com and the others in
    /// [`crate::sync::hosts::BUNDLED`]) cannot be forgotten: a different
    /// key from one of them is always refused.
    pub fn forget_host_key(&mut self) -> Result<SetupState, SetupError> {
        self.expect(&[Step::CheckHost, Step::NeedPin], "forget the host's key")?;
        let address = self.address()?;
        let host = address.connect_host();
        if is_bundled(host, address.port) {
            return Err(failed(bundled_refusal(host)));
        }
        self.known_hosts().forget(host, address.port).map_err(failed)?;
        self.remote_message = None;
        self.go(Step::CheckHost)
    }

    fn remote(&self) -> Result<sync::Remote, SetupError> {
        let address = self.address()?;
        let pin = pin_for(&self.known_hosts(), address.connect_host(), address.port)
            .map_err(failed)?
            .ok_or_else(|| SetupError::WrongStep { message: "the host has not been checked".into() })?;
        Ok(address.remote(pin))
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
            _ if said.as_deref().is_some_and(says_repository_missing) => (
                RemoteReason::RepositoryNotFound,
                failed(format!(
                    "no store at that address: {} says {} doesn't exist, or this key can't see it",
                    remote.host, remote.path
                )),
            ),
            other => (RemoteReason::Other, failed(other)),
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
        let (_, _, host, port) = PORT_FALLBACKS
            .iter()
            .find(|(h, p, _, _)| address.connect_host().eq_ignore_ascii_case(h) && address.port == *p)?;
        let to = StoreAddress {
            user: address.user.clone(),
            host: (*host).to_owned(),
            port: *port,
            path: format!("/{}", address.path.trim_start_matches('/')),
        };
        Some(PortFallback { from_host: address.connect_host().to_owned(), from_port: address.port, host: (*host).to_owned(), port: *port, address: to.display() })
    }

    /// Switches the store's address to [`Self::port_fallback`]'s, keeping
    /// the repository, and goes back to the host check. Refused when none
    /// is offered.
    pub fn use_port_fallback(&mut self) -> Result<SetupState, SetupError> {
        self.expect(&[Step::CheckHost, Step::NeedPin], "use another port")?;
        let Some(fallback) = self.port_fallback() else {
            return Err(SetupError::WrongStep { message: "no other way to reach the host is offered now".into() });
        };
        self.saved.notice = Some(format!(
            "Port {} of {} could not be reached, so paw will use {} on port {} instead.",
            fallback.from_port, fallback.from_host, fallback.host, fallback.port
        ));
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

    /// Clones the store with one login. `pin` is used for that login and
    /// dropped when this returns.
    pub fn download(
        &mut self,
        card: Arc<dyn ApduTransport>,
        pin: &str,
        events: Arc<dyn SetupEvents>,
    ) -> Result<SetupState, SetupError> {
        let (remote, repo) = self.begin_download()?;

        let connection = {
            let clock = self.authentication;
            let asked = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (said, tell) = (asked.clone(), events.clone());
            let signer = CardSigner::new(card, pin, move || {
                said.store(true, std::sync::atomic::Ordering::SeqCst);
                let expectation = clock.expectation();
                if expectation != TouchExpectation::NotNeeded {
                    tell.touch_needed(expectation);
                }
            })
            .map_err(failed);
            let opened = signer.and_then(|signer| {
                Connection::open(&remote, &signer).map_err(|e| match e {
                    e @ (PawError::Network(_) | PawError::HostKeyMismatch { .. } | PawError::KeyRejected) => {
                        self.connection_error(&remote, e, None)
                    }
                    other => key_error(other, asked.load(std::sync::atomic::Ordering::SeqCst)),
                })
            });
            match opened {
                Ok(connection) => connection,
                Err(e) => {
                    self.go(Step::NeedPin)?;
                    return Err(e);
                }
            }
        };
        self.authentication.used_at(Instant::now());

        let mut cancelled = false;
        let mut progress = |received, total| {
            let go_on = events.progress(received, total);
            cancelled |= !go_on;
            go_on
        };
        let cloned = sync::clone_with_progress(&connection, &remote, &repo, Some(&mut progress));
        let said = connection.remote_said();
        drop(connection);
        match cloned {
            // Left as a kill would leave it: `open` cleans up.
            Err(_) if cancelled => Err(SetupError::Cancelled),
            other => self.finish_download(&remote, other, said),
        }
    }

    /// The start of a download: the remote to clone and where to, with any
    /// earlier attempt removed. Setup is at "cloning" until
    /// [`Self::finish_download`] or [`Self::abandon_download`].
    pub(crate) fn begin_download(&mut self) -> Result<(sync::Remote, PathBuf), SetupError> {
        self.expect(&[Step::NeedPin], "download")?;
        let remote = self.remote()?;
        let repo = self.repo_dir();
        remove_dir(&repo)?;
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
        std::fs::write(self.dir.join(REMOTE_FILE), format!("{address}\n")).map_err(failed)?;
        self.go(Step::TestDecrypt)
    }

    /// The repository is not a store paw can use: back to the address.
    fn wrong_repository(&mut self, reason: RemoteReason) -> Result<SetupState, SetupError> {
        remove_dir(&self.repo_dir())?;
        self.saved.opened = None;
        self.note_remote(reason, None);
        self.saved.problem = Some(reason);
        self.go(Step::Address)?;
        Err(match reason {
            RemoteReason::EmptyRepository => SetupError::EmptyRepository,
            _ => SetupError::NotAPassStore,
        })
    }

    /// Opens one entry the key can open, to prove it works. The plaintext is
    /// wiped at once. `pin` is dropped when this returns.
    ///
    /// Then, as after the session's key check: the card's key is pinned and
    /// bound to its `.gpg-id` line, every `.gpg-id` is accepted, and the
    /// key is recorded as the store's enrolled key.
    pub fn test_decrypt(
        &mut self,
        card: Arc<dyn ApduTransport>,
        pin: &str,
        events: Arc<dyn SetupEvents>,
    ) -> Result<SetupState, SetupError> {
        let entry = self.pick_test_entry()?;
        let store = PassStore::open(self.repo_dir().to_string_lossy().into_owned()).map_err(failed)?;
        let ciphertext = store.read_entry(&entry).map_err(failed)?;

        let clock = self.decryption;
        let asked = std::sync::atomic::AtomicBool::new(false);
        let touch = || {
            asked.store(true, std::sync::atomic::Ordering::SeqCst);
            let expectation = clock.expectation();
            if expectation != TouchExpectation::NotNeeded {
                events.touch_needed(expectation);
            }
        };
        match crate::decrypt_entry(card.clone(), pin, &ciphertext, &touch) {
            Ok(plaintext) => drop(Zeroizing::new(plaintext)),
            Err(PawError::NotForThisKey { .. }) => return Err(SetupError::KeyCannotOpenStore),
            Err(e @ (PawError::WrongPin { .. } | PawError::PinBlocked)) => return Err(key_error(e, false)),
            Err(e) if asked.load(std::sync::atomic::Ordering::SeqCst) => return Err(key_error(e, true)),
            Err(e) => return Err(SetupError::CannotOpenEntry { entry, detail: e.to_string() }),
        }
        let info = card_info(card.clone()).map_err(failed)?;
        let key = crate::card_decryption_key(card).map_err(failed)?;
        self.finish_test_decrypt(entry, &key, &KeyIdentity::of(&info))
    }

    /// The entry the test decrypt opens: the first the key surely opens,
    /// else the first it might (hidden recipients).
    pub(crate) fn pick_test_entry(&self) -> Result<String, SetupError> {
        self.expect(&[Step::TestDecrypt], "test the key")?;
        let fingerprint = self.key()?.decryption_fingerprint.clone();
        let store = PassStore::open(self.repo_dir().to_string_lossy().into_owned()).map_err(failed)?;

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
            return Err(SetupError::UnusableKey {
                message: "this is not the key read at the start of setup; plug that one in".into(),
            });
        }
        // The key about to be trusted for every future save must be the very
        // key whose fingerprint the card reported, not merely one handed in
        // alongside it.
        if !crate::write::cert::fingerprint_of(card_key)
            .eq_ignore_ascii_case(&summary.decryption_fingerprint)
        {
            return Err(SetupError::UnusableKey {
                message: "the key read from the card does not match its fingerprint".into(),
            });
        }
        let repo = self.repo_dir();
        let mut keyring = Keyring::open(&self.dir).map_err(write_failed)?;
        let fingerprint = keyring.pin_card_key(card_key).map_err(write_failed)?;
        // Only the `.gpg-id` that governs the entry just opened, and only if
        // that entry names the card's key, says which line is the card's.
        let store = PassStore::open(repo.to_string_lossy().into_owned()).map_err(failed)?;
        let ciphertext = store.read_entry(&entry).map_err(failed)?;
        let bound = keyring.bind_card_for_opened_entry(&repo, &fingerprint, &entry, &ciphertext, crate::write::now_secs());
        match bound {
            // Several unknown lines: the person picks the card's later.
            Ok(_) | Err(WriteError::MissingRecipients { .. }) => {}
            Err(e) => return Err(write_failed(e)),
        }
        keyring.accept_all_recipients_at_setup(&repo).map_err(write_failed)?;
        save_enrolment(&self.dir, identity).map_err(failed)?;
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
        let missing_recipients = if top.writable() {
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
            if !recipients.writable() {
                folders_needing_keys.push(gpg_id);
            }
        }
        Ok(SetupWriteReadiness { can_save: top.writable(), missing_recipients, folders_needing_keys })
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
        .map_err(|e| SetupError::UnusableKey { message: e.to_string() })?;
    // An ssh-ed25519 blob ends in the 32-byte key.
    let raw: [u8; 32] = blob
        .get(blob.len().saturating_sub(32)..)
        .and_then(|tail| tail.try_into().ok())
        .ok_or_else(|| SetupError::UnusableKey { message: "not an ed25519 key".into() })?;
    let key = SshPublicKey::ed25519(&raw);
    if key.blob() != blob.as_slice() {
        return Err(SetupError::UnusableKey { message: "not an ed25519 key".into() });
    }
    Ok(key)
}

/// Why a bundled host's key is not forgotten.
pub(crate) fn bundled_refusal(host: &str) -> String {
    format!(
        "{host}'s keys come with paw and can't be forgotten. A different key means this connection isn't reaching the real {host}: try another network"
    )
}

/// The store address saved by a finished download, for sync.
pub fn saved_address(state_dir: &Path) -> Result<StoreAddress, PawError> {
    let text = std::fs::read_to_string(state_dir.join(REMOTE_FILE))
        .map_err(|e| PawError::Store(format!("no store is set up here: {e}")))?;
    StoreAddress::parse(&text).map(|parsed| parsed.address).map_err(PawError::Store)
}

/// Where the store of `state_dir` is checked out.
pub fn repo_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(REPO_DIR)
}

/// The known-hosts file of `state_dir`.
pub fn known_hosts(state_dir: &Path) -> KnownHosts {
    KnownHosts::new(&state_dir.join(KNOWN_HOSTS))
}

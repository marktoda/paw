//! First-run setup (design 2.4) as a state machine, with no UI in it.
//!
//! Each step is one call. A failure is a [`SetupError`] and leaves the flow
//! at the step that failed, so the same call can be tried again. The step
//! reached is saved in a small file in the state directory after every
//! change, with nothing secret in it. If the app is killed,
//! [`SetupFlow::open`] picks up where it stopped; a download that was
//! interrupted is removed and starts again.


use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::files::values::{self, Values};
use crate::files::{self, layout};
use crate::key::enrolment::save_enrolment;
use crate::key::{CardInfo, KeyError, KeyIdentity, KeyTouch, PinKdfStatus};
use crate::recipients::Access;
use crate::ssh::SshPublicKey;
use crate::store::PassStore;
use crate::sync::address::{AddressNotice, AddressProblem, StoreAddress};
use crate::sync::hosts::{self, KnownHosts};
use crate::sync::{self, Cloned, Connection, PresentedHostKey};
use crate::write::{Keyring, LineStatus, WriteError};
use crate::fault::Fault;
use crate::remote::{PortFallback, RemoteFailure};
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
    #[error("that store is empty")]
    EmptyRepository,
    #[error("that repository isn't a pass store: it has no .gpg-id file at the top")]
    NotAPassStore,
    #[error("cloned, but this key can't open any entry in the store")]
    KeyCannotOpenStore,
    #[error("this key cannot be used: {reason}")]
    UnusableKey { reason: UnusableKeyReason },
    #[error("{message}")]
    WrongStep { message: String },
    /// Underneath the step: the store, the host, the network or the
    /// repository there. Nothing was kept.
    #[error(transparent)]
    Fault { fault: Fault },
}

crate::fault::workflow_error!(SetupError);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Enum)]
pub enum UnusableKeyReason {
    /// Its PIN KDF is one paw cannot follow. No PIN was sent to it.
    /// `detail` says what the card asked for, when known.
    #[error("this key uses a PIN setting (KDF) that paw doesn't support yet. paw has not sent it the PIN, so no try was used")]
    PinKdfUnsupported { detail: Option<String> },
    #[error("it has no authentication key, so it cannot log in to a git host")]
    NoAuthenticationKey,
    /// Its authentication key is `algorithm` ("RSA", …), and paw logs in
    /// with Ed25519 only.
    #[error("its authentication key is {algorithm}, not Ed25519")]
    AuthenticationKeyNotEd25519 { algorithm: String },
    #[error("it has no decryption key")]
    NoDecryptionKey,
    #[error("this is not the key read at the start of setup; plug that one in")]
    NotTheKeyRead,
    #[error("the key read from the card does not match its fingerprint")]
    CardKeyMismatch,
    #[error("{detail}")]
    SshKeyUnreadable { detail: String },
}

fn failed(error: impl std::fmt::Display) -> SetupError {
    Fault::local(error).into()
}

/// Whether new entries can be saved once setup is done. Reading works
/// either way; saving needs a public key on this device for every
/// recipient `.gpg-id` names.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SetupWriteReadiness {
    pub can_save: bool,
    /// Lines of the top-level `.gpg-id` paw has no key for (or cannot tell
    /// which pinned key they mean). Add those keys, or bind the lines, to
    /// save. Empty when `can_save`.
    pub missing_recipients: Vec<String>,
    /// Other `.gpg-id` files (e.g. `archive/.gpg-id`) whose folders cannot
    /// be saved in yet, for the same reason.
    pub folders_needing_keys: Vec<String>,
}

fn write_failed(fault: Fault) -> SetupError {
    match fault {
        Fault::Local { detail } => failed(format!("could not record the store's keys: {detail}")),
        other => other.into(),
    }
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
}

impl Saved {
    fn fresh() -> Self {
        Self { step: Step::ReadKey, key: None, address: None, notice: None, opened: None }
    }

    fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = vec![("step", self.step.name().to_owned())];
        if let Some(key) = &self.key {
            pairs.push(("ident", key.ident.clone()));
            pairs.push(("ssh_public_key", key.ssh_public_key.clone()));
            pairs.push(("decryption_fingerprint", key.decryption_fingerprint.clone()));
            pairs.push(("authentication_touch", touch_name(key.authentication_touch).to_owned()));
            pairs.push(("decryption_touch", touch_name(key.decryption_touch).to_owned()));
        }
        let notice = self.notice.as_ref().map(notice_line);
        for (name, value) in [("address", &self.address), ("notice", &notice), ("opened", &self.opened)] {
            if let Some(value) = value {
                pairs.push((name, value.clone()));
            }
        }
        pairs.into_iter().map(|(name, value)| (name, value.replace(['\n', '\r'], " "))).collect()
    }

    /// Anything unreadable means starting setup again, which is always safe.
    fn from_values(values: &Values) -> Self {
        let value = |name: &str| values.get(name).map(str::to_owned);
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
        };
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
        AddressNotice::PortFallback { fallback: PortFallback { from_host, from_port, host, port } } => {
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
            fallback: PortFallback {
                from_host: text(from_host),
                from_port: from_port.parse().ok()?,
                host: text(host),
                port: port.parse().ok()?,
            },
        },
        _ => return None,
    })
}

pub struct SetupFlow {
    dir: PathBuf,
    saved: Saved,
    /// The key a host presented that is waiting for the person's confirmation.
    /// Not saved: after a restart the host is asked again.
    unconfirmed: Option<PresentedHostKey>,
    authentication: TouchClock,
    decryption: TouchClock,
}

fn remove_dir(path: &Path, _held: &crate::lock::Exclusive) -> Result<(), SetupError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(failed(format!("could not remove the unfinished download: {e}"))),
    }
}

impl SetupFlow {
    pub fn open(state_dir: &Path) -> Result<Self, SetupError> {
        std::fs::create_dir_all(state_dir).map_err(failed)?;
        let saved = match values::read(&state_dir.join(layout::SETUP)).map_err(failed)? {
            Some(found) => Saved::from_values(&found),
            None => Saved::fresh(),
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
        };
        let repo = flow.repo_dir();
        let cloned = repo.join(".git").is_dir() && layout::holds_store(&repo);
        match flow.saved.step {
            // Killed while downloading, or the download went missing.
            Step::Cloning => flow.restart_download()?,
            Step::TestDecrypt | Step::Done if !cloned => flow.restart_download()?,
            _ => {}
        }
        Ok(flow)
    }

    pub fn repo_dir(&self) -> PathBuf {
        crate::files::layout::repo_dir(&self.dir)
    }

    fn known_hosts(&self) -> KnownHosts {
        KnownHosts::of(&self.dir)
    }

    fn save(&self) -> Result<(), SetupError> {
        let pairs = self.saved.pairs();
        let pairs = pairs.iter().map(|(name, value)| (*name, value.as_str()));
        values::write(&self.dir.join(layout::SETUP), ".setup.tmp-", pairs).map_err(failed)
    }

    fn go(&mut self, step: Step) -> Result<SetupState, SetupError> {
        self.saved.step = step;
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

    /// 0 for a store that cannot be read: the count is only shown, and the
    /// step after it (the key check, or opening the store) says what is
    /// wrong.
    fn entry_count(&self) -> u32 {
        PassStore::open(self.repo_dir().to_string_lossy().into_owned())
            .map(|store| store.entry_count())
            .unwrap_or(0)
    }

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
        if self.unconfirmed.is_some() {
            return Err(self.wrong_step(doing));
        }
        self.at(steps, doing)
    }

    fn at(&self, steps: &[Step], doing: &str) -> Result<(), SetupError> {
        if steps.contains(&self.saved.step) { Ok(()) } else { Err(self.wrong_step(doing)) }
    }

    fn wrong_step(&self, doing: &str) -> SetupError {
        SetupError::WrongStep { message: format!("cannot {doing} now: setup is at {}", self.saved.step.name()) }
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
        ssh: Result<&SshPublicKey, KeyError>,
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
        let ssh = ssh.map_err(|error| SetupError::UnusableKey {
            reason: match error {
                KeyError::AuthenticationKeyNotEd25519 { algorithm } => {
                    UnusableKeyReason::AuthenticationKeyNotEd25519 { algorithm }
                }
                _ => UnusableKeyReason::NoAuthenticationKey,
            },
        })?;
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

    pub fn set_address(&mut self, text: &str) -> Result<SetupState, SetupError> {
        self.expect(&[Step::Address, Step::CheckHost], "set the address")?;
        let parsed = StoreAddress::parse(text).map_err(|problem| SetupError::BadAddress { problem })?;
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
        match self.known_hosts().pin(&host, address.port)? {
            Some(_) => self.ask_host(),
            None => {
                self.unconfirmed = Some(sync::scan_host_key(&host, address.port)?);
                Ok(self.state())
            }
        }
    }

    /// The person compared the fingerprint and accepts the host's key.
    pub fn confirm_host(&mut self, fingerprint: &str) -> Result<SetupState, SetupError> {
        self.at(&[Step::CheckHost, Step::NeedPin], "confirm the host")?;
        let address = self.address()?;
        let Some(key) = self.unconfirmed.take() else {
            return Err(SetupError::WrongStep { message: "no host key is waiting to be confirmed".into() });
        };
        let trusted = self.known_hosts().trust(address.connect_host(), address.port, || Ok(key.clone()), fingerprint)?;
        if !trusted {
            self.unconfirmed = Some(key);
            return Err(RemoteFailure::HostKeyNotConfirmed { host: address.host }.into());
        }
        self.ask_host()
    }

    /// "I expected this change": forgets the key pinned for the store's
    /// host, so the next [`Self::check_host`] shows the key it presents now
    /// for confirmation, as on first use. For the changed-host-key screen,
    /// after it has shown both fingerprints; setup stays where it is.
    /// Refused for a host whose keys ship with paw ([`KnownHosts::forget`]).
    pub fn forget_host_key(&mut self) -> Result<SetupState, SetupError> {
        self.expect(&[Step::CheckHost, Step::NeedPin], "forget the host's key")?;
        let address = self.address()?;
        self.known_hosts().forget(address.connect_host(), address.port)?;
        self.go(Step::CheckHost)
    }

    fn remote(&self) -> Result<sync::Remote, SetupError> {
        let address = self.address()?;
        let remote = self.known_hosts().remote(&address)?;
        remote.ok_or_else(|| SetupError::WrongStep { message: "the host has not been checked".into() })
    }

    pub fn use_port_fallback(&mut self) -> Result<SetupState, SetupError> {
        self.expect(&[Step::CheckHost, Step::NeedPin], "use another port")?;
        let address = self.address()?;
        let Some(fallback) = hosts::port_fallback(address.connect_host(), address.port) else {
            return Err(SetupError::WrongStep { message: "no other way to reach the host is offered now".into() });
        };
        self.saved.address = Some(fallback.address(&address).display());
        self.saved.notice = Some(AddressNotice::PortFallback { fallback });
        self.go(Step::CheckHost)
    }

    /// Asks the pinned host whether it accepts the key. No signature.
    fn ask_host(&mut self) -> Result<SetupState, SetupError> {
        let remote = self.remote()?;
        let key = ssh_key(self.key()?)?;
        Connection::probe(&remote, &key)?;
        self.go(Step::NeedPin)
    }

    /// Setup is at "cloning" until
    /// [`Self::finish_download`] or [`Self::abandon_download`].
    pub(crate) fn begin_download(&mut self) -> Result<(sync::Remote, PathBuf), SetupError> {
        self.expect(&[Step::NeedPin], "download")?;
        let remote = self.remote()?;
        let repo = self.repo_dir();
        remove_dir(&repo, &self.exclusive()?)?;
        self.go(Step::Cloning)?;
        Ok((remote, repo))
    }

    /// The download stopped (cancelled, or the login failed): back to the
    /// PIN step, with nothing left of it. Called on the way out of a
    /// failure that is already being reported: if this can't be done too
    /// (the store is busy, the disk refuses), setup stays at "cloning",
    /// which the next [`Self::open`] starts over from.
    pub(crate) fn abandon_download(&mut self) {
        if self.saved.step == Step::Cloning {
            let _ = self.restart_download();
        }
    }

    /// The login signed: a cached touch holds for a while from now.
    pub(crate) fn note_login(&mut self) {
        self.authentication.used_at(Instant::now());
    }

    /// A repository with no commits, or with no `.gpg-id` at the top, is
    /// the wrong address rather than a failed download: setup goes back to
    /// the address, with the address kept.
    pub(crate) fn finish_download(&mut self, cloned: Result<Cloned, Fault>) -> Result<SetupState, SetupError> {
        let repo = self.repo_dir();
        let outcome = match cloned {
            Err(e) => Err(SetupError::from(e)),
            Ok(Cloned::Empty) => return self.wrong_repository(SetupError::EmptyRepository),
            // Nothing was checked out; the download is removed below.
            Ok(Cloned::Refused(paths)) => Err(RemoteFailure::RefusedContent { paths }.into()),
            Ok(Cloned::Store) if layout::holds_store(&repo) => Ok(()),
            Ok(Cloned::Store) => return self.wrong_repository(SetupError::NotAPassStore),
        };
        if let Err(e) = outcome {
            self.restart_download()?;
            return Err(e);
        }
        let address = self.saved.address.clone().unwrap_or_default();
        {
            let _held = self.exclusive()?;
            let remote = self.dir.join(layout::REMOTE);
            files::write_atomic(&remote, ".remote.tmp-", format!("{address}\n").as_bytes()).map_err(failed)?;
        }
        self.go(Step::TestDecrypt)
    }

    fn wrong_repository(&mut self, why: SetupError) -> Result<SetupState, SetupError> {
        remove_dir(&self.repo_dir(), &self.exclusive()?)?;
        self.saved.opened = None;
        self.go(Step::Address)?;
        Err(why)
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
        if !crate::recipients::fingerprint_of(card_key)
            .eq_ignore_ascii_case(&summary.decryption_fingerprint)
        {
            return Err(SetupError::UnusableKey { reason: UnusableKeyReason::CardKeyMismatch });
        }
        let repo = self.repo_dir();
        // The keyring and the enrolment change as one critical section.
        let held = self.exclusive()?;
        let mut keyring = Keyring::open(&self.dir).map_err(write_failed)?;
        let fingerprint = keyring.pin_card_key(&held, card_key).map_err(write_failed)?;
        // Only the `.gpg-id` that governs the entry just opened, and only if
        // that entry names the card's key, says which line is the card's.
        let store = PassStore::open(repo.to_string_lossy().into_owned()).map_err(SetupError::from)?;
        let ciphertext = store.read_entry(&entry).map_err(SetupError::from)?;
        let bound = keyring.bind_card_for_opened_entry(&held, &repo, &fingerprint, &entry, &ciphertext);
        match bound {
            // Several unknown lines: the person picks the card's later.
            Ok(_) | Err(WriteError::MissingRecipients { .. }) => {}
            Err(WriteError::Fault { fault }) => return Err(write_failed(fault)),
            // The card's key was pinned just above.
            Err(other) => return Err(write_failed(Fault::local(other))),
        }
        keyring.accept_all_recipients_at_setup(&held, &repo).map_err(write_failed)?;
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
        let top = keyring.folder_recipients(&repo, "").map_err(write_failed)?;
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
            let folder = crate::path::folder_of_gpg_id(&gpg_id);
            if folder.is_empty() {
                continue;
            }
            let recipients = keyring.folder_recipients(&repo, folder).map_err(write_failed)?;
            if !recipients.writable {
                folders_needing_keys.push(gpg_id);
            }
        }
        Ok(SetupWriteReadiness { can_save: top.writable, missing_recipients, folders_needing_keys })
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
            AddressNotice::PortFallback {
                fallback: PortFallback { from_host: "github.com".into(), from_port: 22, host: "ssh.github.com".into(), port: 443 },
            },
        ];
        for notice in notices {
            assert_eq!(notice_from(&notice_line(&notice)), Some(notice));
        }
        let old = "Port 22 of github.com could not be reached, so paw will use ssh.github.com on port 443 instead.";
        let text = format!("step=check-host\nident=x\nssh_public_key=k\ndecryption_fingerprint=f\naddress=a\nnotice={old}\n");
        let saved = Saved::from_values(&Values::parse(text.into_bytes()).unwrap());
        assert_eq!(saved.step, Step::CheckHost);
        assert_eq!(saved.notice, None);
    }

    #[test]
    fn a_finished_setup_starts_with_the_line_layout_reads() {
        let done = Saved { step: Step::Done, ..Saved::fresh() };
        let pairs = done.pairs();
        let text = values::text(pairs.iter().map(|(name, value)| (*name, value.as_str())));
        assert_eq!(text.lines().next(), Some(layout::SETUP_DONE));
    }

    /// A card that names one key's fingerprint and hands over another key:
    /// the key check pins nothing. The card library already refuses such a
    /// card when the session opens, so only this check reaches it.
    #[cfg(feature = "sim")]
    #[test]
    fn a_card_key_its_fingerprint_does_not_name_is_never_pinned() {
        let card = crate::sim::SimulatedKey::new("123456".into(), false, 1_000);
        let other = crate::sim::SimulatedKey::new_other("123456".into(), false, 1_000);
        let fingerprint = card.fingerprint_hex();
        let dir = tempfile::tempdir().unwrap();
        let mut flow = SetupFlow::open(dir.path()).unwrap();
        let key = KeySummary {
            ident: "0006:1".into(),
            ssh_public_key: card.ssh_public_key(),
            decryption_fingerprint: fingerprint.clone(),
            authentication_touch: KeyTouch::Unknown,
            decryption_touch: KeyTouch::Unknown,
        };
        flow.saved = Saved { step: Step::TestDecrypt, key: Some(key), ..Saved::fresh() };
        let identity =
            KeyIdentity { ident: "0006:1".into(), decryption_fingerprint: Some(fingerprint), authentication_fingerprint: None };

        let error = flow.finish_test_decrypt("web/github.com".into(), &other.decryption_key().unwrap(), &identity).unwrap_err();

        assert!(matches!(error, SetupError::UnusableKey { reason: UnusableKeyReason::CardKeyMismatch }), "{error:?}");
        assert_eq!(flow.saved.step, Step::TestDecrypt);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing written");
    }
}

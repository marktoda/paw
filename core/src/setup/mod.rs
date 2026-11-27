//! First-run setup (design 2.4) as a state machine, with no UI in it.
//!
//! Each step is one call. A failure is a [`SetupError`] and leaves the flow
//! at the step that failed, so the same call can be tried again. The step
//! reached is saved in a small file in the state directory after every
//! change, with nothing secret in it. If the app is killed,
//! [`SetupFlow::open`] picks up where it stopped; a download that was
//! interrupted is removed and starts again.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::files::values::{self, Values};
use crate::fingerprint::Fingerprint;
use crate::lock::StoreDirs;
use crate::files::layout;
use crate::key::enrolment::save_enrolment;
use crate::key::{CardInfo, KeyError, KeyIdentity, KeySession, KeyTouch, PinKdfStatus};
use crate::recipients::Access;
use crate::ssh::SshPublicKey;
use crate::store::PassStore;
use crate::sync::address::{AddressNotice, AddressProblem, StoreAddress, save_address};
use crate::sync::hosts::{self, KnownHosts};
use crate::sync::{self, Cloned, Connection, PresentedHostKey};
use crate::write::{Keyring, LineStatus};
use crate::fault::Fault;
use crate::locked;
use crate::remote::{PortFallback, RemoteFailure};

/// The key as setup shows it on "Your key". Public data only.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeySummary {
    /// Manufacturer and serial, e.g. "0006:12345678".
    pub ident: String,
    /// The SSH public key line to add to the git host.
    pub ssh_public_key: String,
    pub decryption_fingerprint: Fingerprint,
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
    NeedPin { address: String },
    /// Cloned; `test_decrypt_with_key` opens one entry to prove the key works.
    TestDecrypt { entries: u32 },
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
    fn name(self) -> &'static str {
        match self {
            Step::ReadKey => "read-key",
            Step::Address => "address",
            Step::CheckHost => "check-host",
            Step::NeedPin => "need-pin",
            Step::Cloning => "cloning",
            Step::TestDecrypt => "test-decrypt",
            Step::Done => "done",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "read-key" => Step::ReadKey,
            "address" => Step::Address,
            "check-host" => Step::CheckHost,
            "need-pin" => Step::NeedPin,
            "cloning" => Step::Cloning,
            "test-decrypt" => Step::TestDecrypt,
            "done" => Step::Done,
            _ => return None,
        })
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

/// The key as setup read it at "Your key".
#[derive(Debug, Clone)]
struct SetupKey {
    ident: String,
    ssh: SshPublicKey,
    decryption: Fingerprint,
    authentication_touch: KeyTouch,
    decryption_touch: KeyTouch,
}

impl SetupKey {
    fn summary(&self) -> KeySummary {
        KeySummary {
            ident: self.ident.clone(),
            ssh_public_key: self.ssh.authorized_key(),
            decryption_fingerprint: self.decryption.clone(),
            authentication_touch: self.authentication_touch,
            decryption_touch: self.decryption_touch,
        }
    }
}

/// What setup has once the address is set: the key, the address and why
/// paw changed it, if it did.
#[derive(Debug, Clone)]
struct Reached {
    key: SetupKey,
    address: StoreAddress,
    notice: Option<AddressNotice>,
}

/// What is saved between launches: the step reached, with what that step
/// has. Nothing in it is secret.
#[derive(Debug, Clone)]
enum Saved {
    ReadKey,
    Address(SetupKey),
    CheckHost(Reached),
    NeedPin(Reached),
    Cloning(Reached),
    TestDecrypt(Reached),
    /// With the entry the test decrypt opened.
    Done(Reached, String),
}

impl Saved {
    fn step(&self) -> Step {
        match self {
            Saved::ReadKey => Step::ReadKey,
            Saved::Address(_) => Step::Address,
            Saved::CheckHost(_) => Step::CheckHost,
            Saved::NeedPin(_) => Step::NeedPin,
            Saved::Cloning(_) => Step::Cloning,
            Saved::TestDecrypt(_) => Step::TestDecrypt,
            Saved::Done(..) => Step::Done,
        }
    }

    fn reached(&self) -> Option<&Reached> {
        match self {
            Saved::ReadKey | Saved::Address(_) => None,
            Saved::CheckHost(reached)
            | Saved::NeedPin(reached)
            | Saved::Cloning(reached)
            | Saved::TestDecrypt(reached)
            | Saved::Done(reached, _) => Some(reached),
        }
    }

    fn key(&self) -> Option<&SetupKey> {
        match self {
            Saved::Address(key) => Some(key),
            other => other.reached().map(|reached| &reached.key),
        }
    }

    fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = vec![("step", self.step().name().to_owned())];
        if let Some(key) = self.key() {
            pairs.push(("ident", key.ident.clone()));
            pairs.push(("ssh_public_key", key.ssh.authorized_key()));
            pairs.push(("decryption_fingerprint", key.decryption.to_string()));
            pairs.push(("authentication_touch", touch_name(key.authentication_touch).to_owned()));
            pairs.push(("decryption_touch", touch_name(key.decryption_touch).to_owned()));
        }
        if let Some(reached) = self.reached() {
            pairs.push(("address", reached.address.display()));
            if let Some(notice) = &reached.notice {
                pairs.push(("notice", notice_line(notice)));
            }
        }
        if let Saved::Done(_, opened) = self {
            pairs.push(("opened", opened.clone()));
        }
        pairs.into_iter().map(|(name, value)| (name, value.replace(['\n', '\r'], " "))).collect()
    }

    /// What `store`'s setup file says. Anything unreadable, or missing for
    /// the step, means starting setup again, which is always safe.
    fn load(store: &StoreDirs) -> std::io::Result<Self> {
        let found = values::read(&store.state_dir().join(layout::SETUP))?;
        Ok(found.as_ref().and_then(Self::read).unwrap_or(Saved::ReadKey))
    }

    #[cfg(test)]
    fn from_values(values: &Values) -> Self {
        Self::read(values).unwrap_or(Saved::ReadKey)
    }

    fn read(values: &Values) -> Option<Self> {
        let key = || {
            Some(SetupKey {
                ident: values.get("ident")?.to_owned(),
                ssh: SshPublicKey::from_authorized_key(values.get("ssh_public_key")?)?,
                decryption: Fingerprint::parse(values.get("decryption_fingerprint")?)?,
                authentication_touch: values.get("authentication_touch").map_or(KeyTouch::Unknown, touch_from),
                decryption_touch: values.get("decryption_touch").map_or(KeyTouch::Unknown, touch_from),
            })
        };
        let reached = || {
            Some(Reached {
                key: key()?,
                address: StoreAddress::parse(values.get("address")?).ok()?.address,
                notice: values.get("notice").and_then(notice_from),
            })
        };
        Some(match Step::from_name(values.get("step")?)? {
            Step::ReadKey => Saved::ReadKey,
            Step::Address => Saved::Address(key()?),
            Step::CheckHost => Saved::CheckHost(reached()?),
            Step::NeedPin => Saved::NeedPin(reached()?),
            Step::Cloning => Saved::Cloning(reached()?),
            Step::TestDecrypt => Saved::TestDecrypt(reached()?),
            Step::Done => Saved::Done(reached()?, values.get("opened")?.to_owned()),
        })
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

/// [`notice_line`] read back.
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

/// Setup finished for `store` and the store is there, as [`SetupFlow::open`]
/// would find it. Reads files and writes nothing, so the AutoFill
/// extension can ask.
pub fn is_set_up(store: &StoreDirs) -> bool {
    matches!(Saved::load(store), Ok(Saved::Done(..))) && layout::holds_store(store.repo_dir())
}

#[uniffi::export]
pub fn store_is_set_up(state_dir: String) -> bool {
    is_set_up(&StoreDirs::new(Path::new(&state_dir)))
}

pub struct SetupFlow {
    store: StoreDirs,
    saved: Saved,
    /// The entries in the store, counted when it was downloaded or setup
    /// was opened past the download: only shown.
    entries: u32,
    /// The key a host presented that is waiting for the person's
    /// confirmation, and the address it was asked at. Not saved: after a
    /// restart the host is asked again.
    unconfirmed: Option<(StoreAddress, PresentedHostKey)>,
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
        let store = StoreDirs::new(state_dir);
        let saved = Saved::load(&store).map_err(failed)?;
        let mut flow = Self { store, saved, entries: 0, unconfirmed: None };
        let cloned = layout::holds_store(flow.store.repo_dir());
        match &flow.saved {
            // Killed while downloading, or the download went missing.
            Saved::Cloning(reached) => flow.restart_download(reached.clone())?,
            Saved::TestDecrypt(reached) | Saved::Done(reached, _) if !cloned => flow.restart_download(reached.clone())?,
            Saved::TestDecrypt(_) | Saved::Done(..) => flow.entries = flow.entry_count(),
            _ => {}
        }
        Ok(flow)
    }

    fn open_store(&self) -> Result<Arc<PassStore>, SetupError> {
        PassStore::of(self.store.clone()).map_err(SetupError::from)
    }

    fn known_hosts(&self) -> KnownHosts {
        KnownHosts::of(&self.store)
    }

    fn go(&mut self, saved: Saved) -> Result<SetupState, SetupError> {
        self.saved = saved;
        let pairs = self.saved.pairs();
        let pairs = pairs.iter().map(|(name, value)| (*name, value.as_str()));
        values::write(&self.store.state_dir().join(layout::SETUP), pairs).map_err(failed)?;
        Ok(self.state())
    }

    /// The store's lock, held: for removing or replacing the repository,
    /// and for the keyring and enrolment writes.
    fn exclusive(&self) -> Result<crate::lock::Exclusive, SetupError> {
        self.store.exclusive().map_err(SetupError::from)
    }

    fn restart_download(&mut self, reached: Reached) -> Result<(), SetupError> {
        remove_dir(self.store.repo_dir(), &self.exclusive()?)?;
        self.go(Saved::NeedPin(reached)).map(drop)
    }

    /// 0 for a store that cannot be read: the count is only shown, and the
    /// step after it (the key check, or opening the store) says what is
    /// wrong.
    fn entry_count(&self) -> u32 {
        self.open_store()
            .map(|store| store.entry_count())
            .unwrap_or(0)
    }

    pub fn state(&self) -> SetupState {
        if let Some((address, key)) = &self.unconfirmed {
            return SetupState::ConfirmHost {
                host: address.connect_host().to_owned(),
                algorithm: key.algorithm(),
                fingerprint: key.fingerprint(),
            };
        }
        match &self.saved {
            Saved::ReadKey => SetupState::ReadKey,
            Saved::Address(key) => SetupState::Address { key: key.summary() },
            Saved::CheckHost(reached) => {
                SetupState::CheckHost { address: reached.address.display(), notice: reached.notice.clone() }
            }
            Saved::NeedPin(reached) | Saved::Cloning(reached) => {
                SetupState::NeedPin { address: reached.address.display() }
            }
            Saved::TestDecrypt(_) => SetupState::TestDecrypt { entries: self.entries },
            Saved::Done(_, opened) => SetupState::Done { opened: opened.clone(), entries: self.entries },
        }
    }

    fn expect(&self, steps: &[Step], doing: &str) -> Result<(), SetupError> {
        if self.unconfirmed.is_some() {
            return Err(self.wrong_step(doing));
        }
        self.at(steps, doing)
    }

    fn at(&self, steps: &[Step], doing: &str) -> Result<(), SetupError> {
        if steps.contains(&self.saved.step()) { Ok(()) } else { Err(self.wrong_step(doing)) }
    }

    /// What setup has, for `doing` at one of `steps`, which all come after
    /// the address.
    fn reached(&self, steps: &[Step], doing: &str) -> Result<Reached, SetupError> {
        self.expect(steps, doing)?;
        self.saved.reached().cloned().ok_or_else(|| self.wrong_step(doing))
    }

    fn wrong_step(&self, doing: &str) -> SetupError {
        SetupError::WrongStep { message: format!("cannot {doing} now: setup is at {}", self.saved.step().name()) }
    }

    /// Goes one step back: the Back button.
    pub fn back(&mut self) -> Result<SetupState, SetupError> {
        if self.unconfirmed.take().is_some() {
            return Ok(self.state());
        }
        match self.saved.clone() {
            Saved::ReadKey | Saved::Done(..) => Ok(self.state()),
            Saved::Address(_) => self.go(Saved::ReadKey),
            Saved::CheckHost(reached) => self.go(Saved::Address(reached.key)),
            Saved::NeedPin(reached) | Saved::Cloning(reached) => self.go(Saved::CheckHost(reached)),
            Saved::TestDecrypt(reached) => {
                self.restart_download(reached)?;
                Ok(self.state())
            }
        }
    }

    /// [`Setup::read_key_from_session`], with the session's `pin_kdf`.
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
        let Some(decryption) = info.decryption_fingerprint.clone() else {
            return Err(SetupError::UnusableKey { reason: UnusableKeyReason::NoDecryptionKey });
        };
        let ssh = ssh.map_err(|error| SetupError::UnusableKey {
            reason: match error {
                KeyError::AuthenticationKeyNotEd25519 { algorithm } => {
                    UnusableKeyReason::AuthenticationKeyNotEd25519 { algorithm }
                }
                _ => UnusableKeyReason::NoAuthenticationKey,
            },
        })?;
        self.go(Saved::Address(SetupKey {
            ident: info.ident.clone(),
            ssh: ssh.clone(),
            decryption,
            authentication_touch: info.authentication_touch,
            decryption_touch: info.decryption_touch,
        }))
    }

    pub fn set_address(&mut self, text: &str) -> Result<SetupState, SetupError> {
        self.expect(&[Step::Address, Step::CheckHost], "set the address")?;
        let Some(key) = self.saved.key().cloned() else { return Err(self.wrong_step("set the address")) };
        let parsed = StoreAddress::parse(text).map_err(|problem| SetupError::BadAddress { problem })?;
        self.go(Saved::CheckHost(Reached { key, address: parsed.address, notice: parsed.notice }))
    }

    pub fn check_host(&mut self) -> Result<SetupState, SetupError> {
        let reached = self.reached(&[Step::CheckHost, Step::NeedPin], "check the host")?;
        let address = reached.address.clone();
        let host = address.connect_host().to_owned();
        match self.known_hosts().pin(&host, address.port)? {
            Some(_) => self.ask_host(reached),
            None => {
                let presented = sync::scan_host_key(&host, address.port)?;
                self.unconfirmed = Some((address, presented));
                Ok(self.state())
            }
        }
    }

    pub fn confirm_host(&mut self, fingerprint: &str) -> Result<SetupState, SetupError> {
        self.at(&[Step::CheckHost, Step::NeedPin], "confirm the host")?;
        let reached = self.saved.reached().cloned().ok_or_else(|| self.wrong_step("confirm the host"))?;
        let Some((address, key)) = self.unconfirmed.take() else {
            return Err(SetupError::WrongStep { message: "no host key is waiting to be confirmed".into() });
        };
        let trusted = self.known_hosts().trust(&address, || Ok(key.clone()), fingerprint)?;
        if !trusted {
            let host = address.connect_host().to_owned();
            self.unconfirmed = Some((address, key));
            return Err(RemoteFailure::HostKeyNotConfirmed { host }.into());
        }
        self.ask_host(reached)
    }

    pub fn forget_host_key(&mut self) -> Result<SetupState, SetupError> {
        let reached = self.reached(&[Step::CheckHost, Step::NeedPin], "forget the host's key")?;
        self.known_hosts().forget(&self.exclusive()?, &reached.address)?;
        self.go(Saved::CheckHost(reached))
    }

    fn remote(&self, address: &StoreAddress) -> Result<sync::Remote, SetupError> {
        let remote = self.known_hosts().remote(address)?;
        remote.ok_or_else(|| SetupError::WrongStep { message: "the host has not been checked".into() })
    }

    pub fn use_port_fallback(&mut self) -> Result<SetupState, SetupError> {
        let reached = self.reached(&[Step::CheckHost, Step::NeedPin], "use another port")?;
        let address = &reached.address;
        let Some(fallback) = hosts::port_fallback(address.connect_host(), address.port) else {
            return Err(SetupError::WrongStep { message: "no other way to reach the host is offered now".into() });
        };
        let address = fallback.address(address);
        let notice = Some(AddressNotice::PortFallback { fallback });
        self.go(Saved::CheckHost(Reached { address, notice, ..reached }))
    }

    /// Asks the pinned host whether it accepts the key. No signature.
    fn ask_host(&mut self, reached: Reached) -> Result<SetupState, SetupError> {
        let remote = self.remote(&reached.address)?;
        Connection::probe(&remote, &reached.key.ssh)?;
        self.go(Saved::NeedPin(reached))
    }

    /// Setup is at "cloning" until
    /// [`Self::finish_download`] or [`Self::abandon_download`].
    pub(crate) fn begin_download(&mut self) -> Result<(sync::Remote, StoreDirs), SetupError> {
        let reached = self.reached(&[Step::NeedPin], "download")?;
        let remote = self.remote(&reached.address)?;
        remove_dir(self.store.repo_dir(), &self.exclusive()?)?;
        self.go(Saved::Cloning(reached))?;
        Ok((remote, self.store.clone()))
    }

    /// The download stopped (cancelled, or the login failed): back to the
    /// PIN step, with nothing left of it. Called on the way out of a
    /// failure that is already being reported: if this can't be done too
    /// (the store is busy, the disk refuses), setup stays at "cloning",
    /// which the next [`Self::open`] starts over from.
    pub(crate) fn abandon_download(&mut self) {
        if let Saved::Cloning(reached) = &self.saved {
            let _ = self.restart_download(reached.clone());
        }
    }

    /// A repository with no commits, or with no `.gpg-id` at the top, is
    /// the wrong address rather than a failed download: setup goes back to
    /// the address.
    pub(crate) fn finish_download(&mut self, cloned: Result<Cloned, Fault>) -> Result<SetupState, SetupError> {
        let reached = self.reached(&[Step::Cloning], "finish the download")?;
        let outcome = match cloned {
            Err(e) => Err(SetupError::from(e)),
            Ok(Cloned::Empty) => return self.wrong_repository(reached, SetupError::EmptyRepository),
            // Nothing was checked out; the download is removed below.
            Ok(Cloned::Refused(paths)) => Err(RemoteFailure::RefusedContent { paths }.into()),
            Ok(Cloned::Store) if layout::holds_store(self.store.repo_dir()) => Ok(()),
            Ok(Cloned::Store) => return self.wrong_repository(reached, SetupError::NotAPassStore),
        };
        if let Err(e) = outcome {
            self.restart_download(reached)?;
            return Err(e);
        }
        save_address(&self.store, &self.exclusive()?, &reached.address).map_err(failed)?;
        self.entries = self.entry_count();
        self.go(Saved::TestDecrypt(reached))
    }

    fn wrong_repository(&mut self, reached: Reached, why: SetupError) -> Result<SetupState, SetupError> {
        remove_dir(self.store.repo_dir(), &self.exclusive()?)?;
        self.go(Saved::Address(reached.key))?;
        Err(why)
    }

    /// The entry the test decrypt opens: the first the key surely opens,
    /// else the first it might (hidden recipients); with the store it is in.
    pub(crate) fn pick_test_entry(&self) -> Result<(String, Arc<PassStore>), SetupError> {
        let reached = self.reached(&[Step::TestDecrypt], "test the key")?;
        let fingerprint = &reached.key.decryption;
        let store = self.open_store()?;

        let mut maybe = None;
        let mut sure = None;
        for entry in store.entries() {
            match store.key_access(&entry.path, Some(fingerprint)) {
                Access::CanOpen => {
                    sure = Some(entry.path.clone());
                    break;
                }
                Access::MightOpen if maybe.is_none() => maybe = Some(entry.path.clone()),
                _ => {}
            }
        }
        let entry = sure.or(maybe).ok_or(SetupError::KeyCannotOpenStore)?;
        Ok((entry, store))
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
        let reached = self.reached(&[Step::TestDecrypt], "finish the key check")?;
        let read = &reached.key.decryption;
        if identity.decryption_fingerprint.as_ref() != Some(read) {
            return Err(SetupError::UnusableKey { reason: UnusableKeyReason::NotTheKeyRead });
        }
        // The key about to be trusted for every future save must be the very
        // key whose fingerprint the card reported, not merely one handed in
        // alongside it.
        if Fingerprint::of(card_key) != *read {
            return Err(SetupError::UnusableKey { reason: UnusableKeyReason::CardKeyMismatch });
        }
        // The keyring and the enrolment change as one critical section.
        let store = &self.store;
        let held = self.exclusive()?;
        let mut keyring = Keyring::open(store).map_err(write_failed)?;
        let fingerprint = keyring.pin_card_key(&held, card_key).map_err(write_failed)?;
        let ciphertext = crate::store::read::read_entry(store.repo_dir(), &entry).map_err(SetupError::from)?;
        keyring.bind_card_for_opened_entry(&held, &fingerprint, &entry, &ciphertext).map_err(write_failed)?;
        keyring.accept_all_recipients_at_setup(&held).map_err(write_failed)?;
        save_enrolment(store, &held, identity).map_err(SetupError::from)?;
        drop(held);
        self.go(Saved::Done(reached, entry))
    }

    pub fn write_readiness(&self) -> Result<SetupWriteReadiness, SetupError> {
        self.expect(&[Step::Done], "say whether entries can be saved")?;
        let reading = self.store.shared().map_err(SetupError::from)?;
        let keyring = Keyring::open(&self.store).map_err(write_failed)?;
        let top = keyring.folder_recipients(&reading, "").map_err(write_failed)?;
        let missing_recipients = if top.writable {
            Vec::new()
        } else {
            top.lines
                .iter()
                .filter(|line| !matches!(line.status, LineStatus::Ready { .. }))
                .map(|line| line.line.clone())
                .collect()
        };
        Ok(SetupWriteReadiness { can_save: top.writable, missing_recipients })
    }

    /// The key setup read at the start, as an identity to hold a session to.
    pub(crate) fn expected_key(&self) -> Option<KeyIdentity> {
        self.saved.key().map(|key| KeyIdentity {
            ident: key.ident.clone(),
            decryption_fingerprint: Some(key.decryption.clone()),
            authentication_fingerprint: None,
        })
    }
}

/// First-run setup as the setup screens drive it: [`SetupFlow`] behind a
/// lock. Each call is one step and returns the state to show next.
///
/// The steps that need the key run as key tasks on the key port's session
/// (the `*_with_key` calls, beside the other key tasks' workflows in
/// `crate::task::workflows`); one that fails ends `SetupFailed` for the
/// work after the key.
#[derive(uniffi::Object)]
pub struct Setup {
    pub(crate) flow: Arc<Mutex<SetupFlow>>,
}

impl Setup {
    fn flow(&self) -> MutexGuard<'_, SetupFlow> {
        locked(&self.flow)
    }
}

#[uniffi::export]
impl Setup {
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Result<Arc<Self>, SetupError> {
        let flow = SetupFlow::open(&PathBuf::from(state_dir))?;
        Ok(Arc::new(Self { flow: Arc::new(Mutex::new(flow)) }))
    }

    pub fn state(&self) -> SetupState {
        self.flow().state()
    }

    pub fn back(&self) -> Result<SetupState, SetupError> {
        self.flow().back()
    }

    pub fn set_address(&self, address: String) -> Result<SetupState, SetupError> {
        self.flow().set_address(&address)
    }

    /// Connects to the host. A host with a pinned key (github.com ships
    /// with paw, others were confirmed before) has it checked, then is asked
    /// whether it accepts this key, without a signature: no PIN, no touch.
    /// An unknown host leads to `ConfirmHost` instead.
    pub fn check_host(&self) -> Result<SetupState, SetupError> {
        self.flow().check_host()
    }

    /// The person compared the fingerprint `ConfirmHost` showed and accepts
    /// the host's key: pins it and goes on.
    pub fn confirm_host(&self, fingerprint: String) -> Result<SetupState, SetupError> {
        self.flow().confirm_host(&fingerprint)
    }

    /// "Your key", from the plugged-in key's session: what the session read
    /// when the key was plugged in. No PIN, no touch, no card exchange. A
    /// key paw cannot use (an unsupported PIN KDF, no decryption key, no
    /// Ed25519 authentication key) is refused here with `UnusableKey`,
    /// before any PIN is asked.
    pub fn read_key_from_session(&self, session: Arc<KeySession>) -> Result<SetupState, SetupError> {
        self.flow().read_key_from(session.card_info(), session.ssh_key(), session.pin_kdf())
    }

    /// "I expected this change", on the changed-host-key screen: forgets
    /// the host's pinned key without leaving setup; `check_host` then shows
    /// the key it presents now for confirmation. Refused with
    /// `RemoteFailure::HostKeyBundled` for a host whose keys ship with
    /// paw, such as github.com.
    pub fn forget_host_key(&self) -> Result<SetupState, SetupError> {
        self.flow().forget_host_key()
    }

    /// After `RemoteFailure::NoConnection` with a `fallback` (no
    /// connection at all to github.com on port 22): switches the address to
    /// that endpoint and goes back to the host check (`CheckHost`, with a
    /// notice saying why); call `check_host` next. Its keys ship with paw,
    /// so nothing is confirmed. Never used unless the person says so.
    pub fn use_port_fallback(&self) -> Result<SetupState, SetupError> {
        self.flow().use_port_fallback()
    }

    /// Once setup is done: whether new entries can be saved with no
    /// further step, and if not, which recipients paw needs keys for.
    /// Reads the pinned keys and the store; changes nothing.
    pub fn write_readiness(&self) -> Result<SetupWriteReadiness, SetupError> {
        self.flow().write_readiness()
    }
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
        let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";
        let fingerprint = "0123456789ABCDEF0123456789ABCDEF01234567";
        let text = format!(
            "step=check-host\nident=x\nssh_public_key={key}\ndecryption_fingerprint={fingerprint}\naddress=git@example.com:a.git\nnotice={old}\n"
        );
        let saved = Saved::from_values(&Values::parse(text.into_bytes()).unwrap());
        assert_eq!(saved.step(), Step::CheckHost);
        assert_eq!(saved.reached().unwrap().notice, None);
    }

    /// An IPv6 host is named without its brackets, as everywhere else.
    #[cfg(feature = "sim")]
    #[test]
    fn an_unconfirmed_ipv6_host_is_named_without_brackets() {
        let card = crate::sim::SimulatedKey::new("123456".into(), false, 1_000);
        let server = crate::sim::remote::SimRemote::start(card.clone()).unwrap();
        let presented = sync::scan_host_key("127.0.0.1", server.port()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut flow = SetupFlow::open(dir.path()).unwrap();
        let key = SetupKey {
            ident: "0006:1".into(),
            ssh: SshPublicKey::ed25519(&card.authentication_public_key()),
            decryption: Fingerprint::parse(&card.fingerprint_hex()).unwrap(),
            authentication_touch: KeyTouch::Unknown,
            decryption_touch: KeyTouch::Unknown,
        };
        let address = StoreAddress::parse("ssh://git@[::1]:2222/store.git").unwrap().address;
        flow.saved = Saved::CheckHost(Reached { key, address: address.clone(), notice: None });
        flow.unconfirmed = Some((address, presented));

        assert!(matches!(flow.state(), SetupState::ConfirmHost { host, .. } if host == "::1"));
        let error = flow.confirm_host("SHA256:not-it").unwrap_err();
        assert!(
            matches!(&error, SetupError::Fault { fault: Fault::Remote { failure: RemoteFailure::HostKeyNotConfirmed { host } } } if host == "::1"),
            "{error:?}"
        );
    }

    /// A card that names one key's fingerprint and hands over another key:
    /// the key check pins nothing. The card library already refuses such a
    /// card when the session opens, so only this check reaches it.
    #[cfg(feature = "sim")]
    #[test]
    fn a_card_key_its_fingerprint_does_not_name_is_never_pinned() {
        let card = crate::sim::SimulatedKey::new("123456".into(), false, 1_000);
        let other = crate::sim::SimulatedKey::new_other("123456".into(), false, 1_000);
        let fingerprint = Fingerprint::parse(&card.fingerprint_hex()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut flow = SetupFlow::open(dir.path()).unwrap();
        let key = SetupKey {
            ident: "0006:1".into(),
            ssh: SshPublicKey::ed25519(&card.authentication_public_key()),
            decryption: fingerprint.clone(),
            authentication_touch: KeyTouch::Unknown,
            decryption_touch: KeyTouch::Unknown,
        };
        let address = StoreAddress::parse("git@example.com:store.git").unwrap().address;
        flow.saved = Saved::TestDecrypt(Reached { key, address, notice: None });
        let identity =
            KeyIdentity { ident: "0006:1".into(), decryption_fingerprint: Some(fingerprint), authentication_fingerprint: None };

        let error = flow.finish_test_decrypt("web/github.com".into(), &other.decryption_key().unwrap(), &identity).unwrap_err();

        assert!(matches!(error, SetupError::UnusableKey { reason: UnusableKeyReason::CardKeyMismatch }), "{error:?}");
        assert_eq!(flow.saved.step(), Step::TestDecrypt);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing written");
    }
}

//! A plugged-in simulated key, a fixture store and a listener that records
//! every stage, for the key session tests; and the key as other tests use
//! it, through a session: a signer for git logins, and a decrypt.

// Each test binary uses its own part of this.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use paw_core::copies::CopyCache;
use paw_core::key::{KeyError, KeyPort};
use paw_core::task::LoginPurpose;
use paw_core::task::{KeyTask, KeyTaskState, TaskFailure};
use paw_core::sim::copies::{SimClock, SimKeystore};
use paw_core::sim::fixture::write_fixture_store;
pub use paw_core::sim::tasks::{SimDriver as Driver, WAIT};
use paw_core::sim::tasks::{SimLogin, SimSigner, decipher, ending, wait_final};
use paw_core::sim::{SimOptions, SimTouch, SimulatedKey};
use paw_core::key::{SignFailure, SshSigner};
use paw_core::ssh::SshPublicKey;
use paw_core::store::PassStore;

pub const PIN: &str = "735190";
/// The password line of [`ENTRY`] in the fixture store.
pub const SECRET: &str = "tidal-Quartz-88-ferns";
pub const ENTRY: &str = "web/github.com";
pub const OTHER_ENTRY: &str = "phone-pin";
/// Encrypted for a key the simulated card does not hold.
pub const FOREIGN_ENTRY: &str = "archive/old-forum";

pub struct Rig {
    pub sim: Arc<SimulatedKey>,
    pub clock: Arc<SimClock>,
    pub port: Arc<KeyPort>,
    pub store: Arc<PassStore>,
    pub copies: Arc<CopyCache>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

pub fn sim(decrypt: SimTouch, auth: SimTouch, touch_timeout: Duration) -> Arc<SimulatedKey> {
    let sim = SimulatedKey::with_options(SimOptions {
        pin: PIN.into(),
        decrypt_touch: false,
        auth_touch: false,
        touch_timeout,
        extended_length: false,
    });
    sim.set_decryption_touch(decrypt);
    sim.set_authentication_touch(auth);
    sim
}

/// A key whose slots need a touch for every use, not yet plugged in.
pub fn rig_unplugged(decrypt: SimTouch, auth: SimTouch) -> Rig {
    let store_dir = tempfile::tempdir().unwrap();
    write_fixture_store(store_dir.path().to_string_lossy().into_owned()).unwrap();
    let store = PassStore::open(store_dir.path().to_string_lossy().into_owned()).unwrap();
    let copies_dir = tempfile::tempdir().unwrap();
    let clock = SimClock::new(1_790_000_000_000);
    let copies = CopyCache::new(
        copies_dir.path().join("window").to_string_lossy().into_owned(),
        SimKeystore::new(),
        clock.clone(),
    );
    Rig {
        sim: sim(decrypt, auth, Duration::from_secs(5)),
        port: KeyPort::new(clock.clone()),
        clock,
        store,
        copies,
        _dirs: (store_dir, copies_dir),
    }
}

impl Rig {
    /// Where the fixture store is, to add crafted entries to it.
    pub fn store_dir(&self) -> &std::path::Path {
        self._dirs.0.path()
    }
}

/// `ciphertext` (one recipient packet, then the encrypted data) with that
/// packet repeated so there are `copies` of it, all but the last damaged
/// so the card's answer does not unwrap them: what a crafted entry that
/// asks the card again and again looks like.
pub fn with_packet_repeated(ciphertext: &[u8], copies: usize) -> Vec<u8> {
    let first = ciphertext[0];
    let (header, length) = if first & 0x40 != 0 {
        match ciphertext[1] {
            n @ 0..192 => (2, n as usize),
            n @ 192..224 => (3, ((n as usize - 192) << 8) + ciphertext[2] as usize + 192),
            255 => (6, u32::from_be_bytes(ciphertext[2..6].try_into().unwrap()) as usize),
            other => panic!("partial length {other} in a recipient packet"),
        }
    } else {
        match first & 3 {
            0 => (2, ciphertext[1] as usize),
            1 => (3, u16::from_be_bytes([ciphertext[1], ciphertext[2]]) as usize),
            _ => (5, u32::from_be_bytes(ciphertext[1..5].try_into().unwrap()) as usize),
        }
    };
    let tag = if first & 0x40 != 0 { first & 0x3f } else { (first >> 2) & 0x0f };
    assert_eq!(tag, 1, "the entry does not start with a recipient packet");
    let packet = &ciphertext[..header + length];
    let mut damaged = packet.to_vec();
    *damaged.last_mut().unwrap() ^= 0x55;
    let mut out = Vec::new();
    for _ in 1..copies {
        out.extend_from_slice(&damaged);
    }
    out.extend_from_slice(ciphertext);
    out
}

pub fn rig(decrypt: SimTouch, auth: SimTouch) -> Rig {
    let rig = rig_unplugged(decrypt, auth);
    rig.port.plugged_in(rig.sim.clone()).unwrap();
    rig
}

pub fn finish(task: &Arc<KeyTask>) -> KeyTaskState {
    wait_final(task, WAIT).expect("the task never finished")
}

pub fn is_pin(state: &KeyTaskState) -> bool {
    matches!(state, KeyTaskState::Pin { .. })
}

pub fn is_touch(state: &KeyTaskState) -> bool {
    matches!(state, KeyTaskState::Touch)
}

pub fn is_busy(state: &KeyTaskState) -> bool {
    matches!(state, KeyTaskState::Busy)
}

pub fn is_done(state: &KeyTaskState) -> bool {
    matches!(state, KeyTaskState::Done { .. })
}

/// Starts opening `entry`, with no Face ID copy, driven by `driver`.
pub fn open(rig: &Rig, entry: &str, driver: &Arc<Driver>) -> Arc<KeyTask> {
    let task = rig.port.clone().open_entry(rig.store.clone(), entry.into(), None, 0, driver.clone());
    driver.drive(&task);
    task
}

/// Waits until `check` holds, for things no listener reports.
pub fn eventually(check: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

/// A port of its own with `sim` plugged in.
pub fn port_with(sim: &Arc<SimulatedKey>) -> Arc<KeyPort> {
    let port = KeyPort::new(SimClock::new(1_790_000_000_000));
    port.plugged_in(sim.clone()).unwrap();
    port
}

/// A sign-in on `port` whose signer the test logs in with, driven by
/// `driver`.
pub fn login(port: &Arc<KeyPort>, purpose: LoginPurpose, driver: &Arc<Driver>) -> SimLogin {
    let login = SimLogin::start(port, purpose, driver.clone());
    driver.drive(login.task());
    login
}

/// `sim` signed in on a port of its own, as the app's sign-in task does:
/// the SSH signer git logins are made with. Typing `pin` and touching the
/// key when asked; the sign-in ends when this is dropped.
pub struct KeySigner {
    pub port: Arc<KeyPort>,
    pub driver: Arc<Driver>,
    signer: SimSigner,
    _login: SimLogin,
}

pub fn signer_with(sim: &Arc<SimulatedKey>, pin: &str) -> KeySigner {
    let port = port_with(sim);
    let driver = Driver::helpful(pin, sim);
    let login = login(&port, LoginPurpose::Fetch, &driver);
    let signer = login.signer().expect("the sign-in failed");
    KeySigner { port, driver, signer, _login: login }
}

/// A sync's failure that is the sync's, not the key's.
pub fn sync_failure(failure: paw_core::task::TaskFailure) -> paw_core::sync::SyncError {
    match failure {
        paw_core::task::TaskFailure::Sync(error) => error,
        other => panic!("expected the sync's failure, got {other:?}"),
    }
}

/// [`signer_with`] for a key whose PIN is [`PIN`].
pub fn signer(sim: &Arc<SimulatedKey>) -> KeySigner {
    signer_with(sim, PIN)
}

impl SshSigner for KeySigner {
    fn public_key(&self) -> &SshPublicKey {
        self.signer.public_key()
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, SignFailure> {
        self.signer.sign(data)
    }
}

/// Deciphers `ciphertext` with the key on `port` in a key task, typing
/// `pin` and touching `sim` when asked.
pub fn decrypt_on(port: &Arc<KeyPort>, sim: &Arc<SimulatedKey>, pin: &str, ciphertext: &[u8]) -> Result<Vec<u8>, KeyError> {
    let driver = Driver::helpful(pin, sim);
    let (task, text) = decipher(port, ciphertext.to_vec(), driver.clone());
    driver.drive(&task);
    match ending(finish(&task)) {
        Ok(_) => Ok(text.recv().unwrap().to_vec()),
        Err(TaskFailure::Key(error)) => Err(error),
        Err(other) => panic!("the decrypt ended {other:?}"),
    }
}

/// [`decrypt_on`] on a port of its own: a key plugged in for this alone.
pub fn decrypt(sim: &Arc<SimulatedKey>, pin: &str, ciphertext: &[u8]) -> Result<Vec<u8>, KeyError> {
    decrypt_on(&port_with(sim), sim, pin, ciphertext)
}

/// [`decrypt`] of an entry that must open, as text.
pub fn decrypt_text(sim: &Arc<SimulatedKey>, pin: &str, ciphertext: &[u8]) -> String {
    String::from_utf8(decrypt(sim, pin, ciphertext).unwrap()).unwrap()
}

//! Stand-ins for the Secure Enclave and the system clocks, for tests and
//! the simulator.
//!
//! [`SimKeystore`] really encrypts (XChaCha20-Poly1305 with a key held in
//! memory), so what it seals is as opaque on disk as on a device, and a
//! reset really makes old blobs unopenable. Face ID's answer is a switch.
//! Like [`super::SimulatedKey`], neither may reach a device build
//! (design.md 5.5).

use std::sync::{Arc, Mutex};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use rand::rngs::OsRng;
use zeroize::Zeroizing;

use crate::locked;
use crate::clock::Clock;
use crate::copies::{DeviceKeystore, KeystoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SimFaceId {
    Match,
    NoMatch,
    Cancel,
    LockedOut,
    NotEnrolled,
}


fn random_key() -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(&mut *key);
    key
}

const NONCE_LEN: usize = 24;

struct KeystoreState {
    seal_key: Zeroizing<[u8; 32]>,
    index_key: Zeroizing<[u8; 32]>,
    face_id: SimFaceId,
    /// Enrolment changed since the key was made: it can never be used.
    invalidated: bool,
    prompts: u32,
    resets: u32,
}

#[derive(uniffi::Object)]
pub struct SimKeystore {
    state: Mutex<KeystoreState>,
}

#[uniffi::export]
impl SimKeystore {
    /// A keystore whose Face ID matches until told otherwise.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(KeystoreState {
                seal_key: random_key(),
                index_key: random_key(),
                face_id: SimFaceId::Match,
                invalidated: false,
                prompts: 0,
                resets: 0,
            }),
        })
    }

    pub fn set_face_id(&self, answer: SimFaceId) {
        locked(&self.state).face_id = answer;
    }

    pub fn reset_count(&self) -> u32 {
        locked(&self.state).resets
    }

    /// This keystore as the Secure Enclave seam, to hand straight to
    /// `CopyCache` with no Swift adapter.
    pub fn as_keystore(self: Arc<Self>) -> Arc<dyn DeviceKeystore> {
        self
    }

}

impl SimKeystore {
    /// A face is added or removed in Settings: the seal key is invalidated
    /// until [`Self::reset`] makes a new one.
    pub fn change_enrolment(&self) {
        locked(&self.state).invalidated = true;
    }

    /// How many times Face ID was shown (each `open` that reached a
    /// working key).
    pub fn prompt_count(&self) -> u32 {
        locked(&self.state).prompts
    }

}

impl DeviceKeystore for SimKeystore {
    fn seal(&self, secret: Vec<u8>) -> Result<Vec<u8>, KeystoreError> {
        let secret = Zeroizing::new(secret);
        let state = locked(&self.state);
        if state.invalidated {
            return Err(KeystoreError::KeyGone);
        }
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let sealed = XChaCha20Poly1305::new((&*state.seal_key).into())
            .encrypt(XNonce::from_slice(&nonce), secret.as_slice())
            .map_err(|_| KeystoreError::Failed { message: "could not seal".into() })?;
        Ok([nonce.as_slice(), &sealed].concat())
    }

    fn unseal(&self, sealed: Vec<u8>, _reason: String) -> Result<Vec<u8>, KeystoreError> {
        let mut state = locked(&self.state);
        if state.invalidated {
            return Err(KeystoreError::KeyGone);
        }
        match state.face_id {
            SimFaceId::LockedOut => return Err(KeystoreError::LockedOut),
            SimFaceId::NotEnrolled => return Err(KeystoreError::NotEnrolled),
            _ => {}
        }
        state.prompts += 1;
        match state.face_id {
            SimFaceId::Cancel => return Err(KeystoreError::Cancelled),
            SimFaceId::NoMatch => return Err(KeystoreError::NotRecognised),
            _ => {}
        }
        let unopenable = || KeystoreError::Failed { message: "not sealed by this key".into() };
        if sealed.len() < NONCE_LEN {
            return Err(unopenable());
        }
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        XChaCha20Poly1305::new((&*state.seal_key).into())
            .decrypt(XNonce::from_slice(nonce), body)
            .map_err(|_| unopenable())
    }

    fn index_key(&self) -> Result<Vec<u8>, KeystoreError> {
        Ok(locked(&self.state).index_key.to_vec())
    }

    /// Makes new keys: nothing sealed before can be opened.
    fn reset(&self) -> Result<(), KeystoreError> {
        let mut state = locked(&self.state);
        state.seal_key = random_key();
        state.index_key = random_key();
        state.invalidated = false;
        state.resets += 1;
        Ok(())
    }
}

struct ClockState {
    wall_ms: i64,
    monotonic_ms: i64,
    boot: u32,
}

/// Clocks that move only when told to.
#[derive(uniffi::Object)]
pub struct SimClock {
    state: Mutex<ClockState>,
}

#[uniffi::export]
impl SimClock {
    /// Starts at `wall_ms`, a minute after boot.
    #[uniffi::constructor]
    pub fn new(wall_ms: i64) -> Arc<Self> {
        Arc::new(Self { state: Mutex::new(ClockState { wall_ms, monotonic_ms: 60_000, boot: 1 }) })
    }

    /// Time passes: both clocks move on.
    pub fn advance_ms(&self, ms: u64) {
        let ms = i64::try_from(ms).unwrap_or(i64::MAX);
        let mut state = locked(&self.state);
        state.wall_ms = state.wall_ms.saturating_add(ms);
        state.monotonic_ms = state.monotonic_ms.saturating_add(ms);
    }

    /// These clocks as the clock seam, to hand straight to `CopyCache` and
    /// `KeyPort` with no Swift adapter.
    pub fn as_clock(self: Arc<Self>) -> Arc<dyn Clock> {
        self
    }
}

impl SimClock {
    /// The person sets the clock: only the wall clock moves.
    pub fn set_wall_ms(&self, wall_ms: i64) {
        locked(&self.state).wall_ms = wall_ms;
    }

    /// The phone restarts: a new boot id and the monotonic clock from zero.
    pub fn reboot(&self) {
        let mut state = locked(&self.state);
        state.boot += 1;
        state.monotonic_ms = 0;
    }
}

impl Clock for SimClock {
    fn wall_ms(&self) -> i64 {
        locked(&self.state).wall_ms
    }

    fn monotonic_ms(&self) -> i64 {
        locked(&self.state).monotonic_ms
    }

    fn boot_id(&self) -> String {
        format!("sim-boot-{}", locked(&self.state).boot)
    }
}

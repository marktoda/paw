//! A simulated OpenPGP card, for tests and for running the app in the iOS
//! simulator, where no real key can be attached.
//!
//! It implements the minimum of OpenPGP card 3.4 needed to read status,
//! verify the user PIN, run ECDH with a Curve25519 key and sign with an
//! ed25519 authentication key (the one SSH uses). Its private keys are fixed,
//! public test values: they protect nothing.
//!
//! Commands may arrive as short APDUs, chained short APDUs or extended-length
//! APDUs; which of the last two the card advertises is an option, because a
//! real key advertises both and the library's choice depends on it.

pub mod copies;
pub mod fixture;
pub mod github;
pub mod prepared;
pub mod remote;
pub mod tasks;

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ed25519_dalek::{Signer, SigningKey};
use sha1::{Digest, Sha1};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::key::FAST_REFUSAL;
use crate::key::apdu::{
    self, DECIPHER_P1, DECIPHER_P2, FORGET_P1, GET_DATA, INTERNAL_AUTHENTICATE, PSO, PW1_82, SELECT, VERIFY, VERIFY_P1,
};
use crate::key::{CardConnection, ConnectionError};

const CREATED: u32 = 1_780_000_000;
const OID_CV25519: [u8; 10] = [0x2B, 0x06, 0x01, 0x04, 0x01, 0x97, 0x55, 0x01, 0x05, 0x01];
const OID_ED25519: [u8; 9] = [0x2B, 0x06, 0x01, 0x04, 0x01, 0xDA, 0x47, 0x0F, 0x01];

/// Largest command the card claims to accept when it advertises extended
/// length. A YubiKey 5 reports a value of this order.
const MAX_EXTENDED_COMMAND: u16 = 2048;

/// How long one touch lasts for a slot with the CACHED policy, unless a test
/// says otherwise. A YubiKey's is 15 seconds.
const TOUCH_CACHE: Duration = Duration::from_secs(15);

const MIN_TOUCH_TIMEOUT: Duration = FAST_REFUSAL.saturating_add(Duration::from_millis(100));

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SimTouch {
    Off,
    /// A touch for every use (YubiKey "on").
    EachUse,
    /// One touch covers every use of the slot for a while (YubiKey "cached").
    Cached,
}

impl SimTouch {
    fn from_flag(touch: bool) -> Self {
        if touch { Self::EachUse } else { Self::Off }
    }

    /// The first byte of the slot's user interaction flag data object.
    fn uif(self) -> u8 {
        match self {
            Self::Off => 0x00,
            Self::EachUse => 0x01,
            Self::Cached => 0x03,
        }
    }
}

/// The fixed, public key material of a simulated key: the byte each
/// private key repeats, and the serial.
#[derive(Clone, Copy)]
struct SimKeys {
    decryption: u8,
    authentication: u8,
    serial: [u8; 4],
}

/// The usual simulated key, which the fixture store is encrypted for.
const KEYS: SimKeys = SimKeys { decryption: 7, authentication: 9, serial: [0x12, 0x34, 0x56, 0x78] };
/// A second simulated key. Its decryption key is not the fixture's other
/// key (that one is `[8; 32]`), so it opens nothing in the fixture store.
const OTHER_KEYS: SimKeys = SimKeys { decryption: 11, authentication: 13, serial: [0x87, 0x65, 0x43, 0x21] };

#[derive(Clone, Copy)]
enum Slot {
    Decryption = 0,
    Authentication = 1,
}

pub struct SimOptions {
    pub pin: String,
    pub decrypt_touch: bool,
    /// A real key sets the touch policy per slot.
    pub auth_touch: bool,
    /// How long a key operation waits for [`SimulatedKey::touch`] before
    /// failing, as a real key does when nobody touches it.
    pub touch_timeout: Duration,
    pub extended_length: bool,
}

impl SimOptions {
    /// One touch policy for both slots, and short APDUs.
    fn new(pin: String, touch_required: bool, touch_timeout_ms: u32) -> Self {
        Self {
            pin,
            decrypt_touch: touch_required,
            auth_touch: touch_required,
            touch_timeout: Duration::from_millis(touch_timeout_ms.into()),
            extended_length: false,
        }
    }
}

#[derive(uniffi::Object)]
pub struct SimulatedKey {
    secret: StaticSecret,
    auth_key: SigningKey,
    serial: [u8; 4],
    options: SimOptions,
    state: Mutex<State>,
    touched: Condvar,
}

struct State {
    pin: String,
    tries_left: u8,
    verified: bool,
    touch_pending: bool,
    deciphers: u32,
    authentications: u32,
    touches: u32,
    chain: Vec<u8>,
    present: bool,
    /// Counts plug-ins, so a command that was running across a removal and
    /// re-insertion still fails.
    plugged: u32,
    reselect_drops_pin: bool,
    /// Indexed by [`Slot`].
    policies: [SimTouch; 2],
    last_touch: [Option<Instant>; 2],
    touch_cache: Duration,
    waiting_for_touch: bool,
    time_out_now: bool,
    /// VERIFY commands with a PIN in them: each may use a try.
    verify_attempts: u32,
    /// Empty VERIFY commands: "is the PIN verified?", which uses no try.
    pin_queries: u32,
    /// The PIN KDF data object, if KDF is on: VERIFY then expects the
    /// derived PIN.
    kdf: Option<Vec<u8>>,
}

fn tlv(tag: &[u8], value: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    match value.len() {
        n if n < 0x80 => out.push(n as u8),
        n if n < 0x100 => out.extend([0x81, n as u8]),
        n => out.extend([0x82, (n >> 8) as u8, n as u8]),
    }
    out.extend_from_slice(value);
    out
}

#[uniffi::export]
impl SimulatedKey {
    /// `touch_timeout_ms` is how long a key operation waits for
    /// [`Self::touch`] before failing, as a real key does when nobody
    /// touches it.
    #[uniffi::constructor]
    pub fn new(pin: String, touch_required: bool, touch_timeout_ms: u32) -> Arc<Self> {
        Self::with_options(SimOptions::new(pin, touch_required, touch_timeout_ms))
    }

    /// A second, different simulated key (other key material and serial),
    /// for the "different key" and "replace my key" screens. It opens
    /// nothing in the fixture store.
    #[uniffi::constructor]
    pub fn new_other(pin: String, touch_required: bool, touch_timeout_ms: u32) -> Arc<Self> {
        Self::other_with_options(SimOptions::new(pin, touch_required, touch_timeout_ms))
    }

    /// Stands in for a finger on the gold disc. One touch allows one key
    /// operation (a decrypt or an authentication signature).
    pub fn touch(&self) {
        self.lock().touch_pending = true;
        self.touched.notify_all();
    }

    /// The key operation waiting for a touch fails now, as it would once
    /// the key's own timeout ran out, so a test about the timeout need not
    /// wait for it. Does nothing if no operation is waiting.
    pub fn time_out_touch(&self) {
        let mut state = self.lock();
        if state.waiting_for_touch {
            state.time_out_now = true;
        }
        drop(state);
        self.touched.notify_all();
    }

    /// Pins this key's decryption key in the store set up in `state_dir`,
    /// as setup pins the card's: a key paw then has the public half of.
    /// Returns its fingerprint.
    pub fn pin_as_card(self: Arc<Self>, state_dir: String) -> Result<String, crate::write::WriteError> {
        let key = self.decryption_key()?;
        let store = crate::lock::StoreDirs::new(std::path::Path::new(&state_dir));
        let held = store.exclusive()?;
        Ok(crate::write::Keyring::open(&store)?.pin_card_key(&held, &key)?.to_string())
    }

    /// Hex OpenPGP v4 fingerprint of the decryption key.
    pub fn fingerprint_hex(&self) -> String {
        crate::fingerprint::hex(&self.fingerprint())
    }

    /// The key is pulled out: every exchange fails, and a key operation
    /// waiting for a touch fails at once.
    pub fn unplug(&self) {
        let mut state = self.lock();
        state.present = false;
        state.verified = false;
        state.touch_pending = false;
        state.chain.clear();
        drop(state);
        self.touched.notify_all();
    }

    /// The key is plugged back in. Like any power cycle, the PIN is no
    /// longer verified and no touch is remembered.
    pub fn plug_in(&self) {
        let mut state = self.lock();
        state.present = true;
        state.plugged += 1;
        state.verified = false;
        state.touch_pending = false;
        state.last_touch = [None, None];
        state.chain.clear();
    }

    /// Another computer has used PIN tries (or reset them).
    pub fn set_pin_tries_left(&self, tries: u8) {
        self.lock().tries_left = tries;
    }

    /// Turns the PIN KDF on, as `ykman openpgp access set-kdf` would
    /// (iterated and salted SHA-256, 100,000 bytes); or, with
    /// `algorithm` other than 3, sets a KDF paw does not support.
    pub fn set_pin_kdf(&self, algorithm: u8) {
        let mut object = vec![0x81, 0x01, algorithm, 0x82, 0x01, 0x08, 0x83, 0x04];
        object.extend(100_000u32.to_be_bytes());
        object.extend([0x84, 0x08]);
        object.extend(b"01234567");
        self.lock().kdf = Some(object);
    }

    pub fn waiting_for_touch(&self) -> bool {
        self.lock().waiting_for_touch
    }

    pub fn pin_tries_left(&self) -> u8 {
        self.lock().tries_left
    }

    /// The key as a card connection, to hand to `KeyPort.plugged_in` with
    /// no Swift adapter.
    pub fn as_connection(self: Arc<Self>) -> Arc<dyn CardConnection> {
        self
    }
}

impl SimulatedKey {
    pub fn decipher_count(&self) -> u32 {
        self.lock().deciphers
    }

    /// Off unless set: whether a YubiKey drops the verified PIN on a SELECT
    /// through the iPhone's reader is not known.
    pub fn set_reselect_drops_pin(&self, drops: bool) {
        self.lock().reselect_drops_pin = drops;
    }

    /// The PIN is changed on another computer. The key was unplugged to get
    /// there, so its PIN is no longer verified either.
    pub fn change_pin_elsewhere(&self, new_pin: String) {
        let mut state = self.lock();
        state.pin = new_pin;
        state.verified = false;
    }

    pub fn set_decryption_touch(&self, policy: SimTouch) {
        self.lock().policies[Slot::Decryption as usize] = policy;
    }

    pub fn set_authentication_touch(&self, policy: SimTouch) {
        self.lock().policies[Slot::Authentication as usize] = policy;
    }

    pub fn set_touch_cache_ms(&self, ms: u32) {
        self.lock().touch_cache = Duration::from_millis(ms.into());
    }

    pub fn verify_attempts(&self) -> u32 {
        self.lock().verify_attempts
    }

    pub fn pin_queries(&self) -> u32 {
        self.lock().pin_queries
    }

    pub fn pin_verified(&self) -> bool {
        self.lock().verified
    }

    /// Touches the key waited for and used, over both slots. A use covered
    /// by a cached touch adds none.
    pub fn touch_count(&self) -> u32 {
        self.lock().touches
    }

    /// The SSH public key line of the authentication slot, as setup shows
    /// it for adding to the git host.
    pub fn ssh_public_key(&self) -> String {
        crate::ssh::SshPublicKey::ed25519(&self.authentication_public_key()).authorized_key()
    }
}

impl CardConnection for SimulatedKey {
    fn transmit(&self, apdu: Vec<u8>) -> Result<Vec<u8>, ConnectionError> {
        self.transmit_checked(&apdu).map_err(|message| ConnectionError::Failed { message })
    }
}

/// OpenPGP v4 fingerprint of a Curve25519 key: `algorithm` is 18 (ECDH) or
/// 22 (EdDSA), `trailer` the ECDH KDF parameters or nothing.
fn fingerprint_v4(algorithm: u8, oid: &[u8], public: [u8; 32], trailer: &[u8]) -> [u8; 20] {
    let mut body = vec![4];
    body.extend(CREATED.to_be_bytes());
    body.push(algorithm);
    body.push(oid.len() as u8);
    body.extend(oid);
    body.extend(263u16.to_be_bytes());
    body.push(0x40);
    body.extend(public);
    body.extend(trailer);

    let mut hasher = Sha1::new();
    hasher.update([0x99]);
    hasher.update((body.len() as u16).to_be_bytes());
    hasher.update(&body);
    hasher.finalize().into()
}

/// The data field of a command APDU, given everything after the four header
/// bytes. Accepts the short and the extended encoding of Lc, with or without
/// a trailing Le. `None` if the lengths do not add up.
fn command_data(body: &[u8]) -> Option<&[u8]> {
    match body {
        [] | [_] | [0, _, _] => Some(&[]),
        [0, hi, lo, rest @ ..] => {
            let lc = usize::from(u16::from_be_bytes([*hi, *lo]));
            (rest.len() == lc || rest.len() == lc + 2).then(|| &rest[..lc])
        }
        [lc, rest @ ..] => {
            let lc = usize::from(*lc);
            (rest.len() == lc || rest.len() == lc + 1).then(|| &rest[..lc])
        }
    }
}

impl SimulatedKey {
    pub fn with_options(options: SimOptions) -> Arc<Self> {
        Self::with_keys(options, KEYS)
    }

    /// A simulated key with other key material and serial: a second
    /// YubiKey, for "this is a different key" and "replace my key".
    pub fn other_with_options(options: SimOptions) -> Arc<Self> {
        Self::with_keys(options, OTHER_KEYS)
    }

    fn with_keys(options: SimOptions, keys: SimKeys) -> Arc<Self> {
        Arc::new(Self {
            secret: StaticSecret::from([keys.decryption; 32]),
            auth_key: SigningKey::from_bytes(&[keys.authentication; 32]),
            serial: keys.serial,
            state: Mutex::new(State {
                pin: options.pin.clone(),
                tries_left: 3,
                verified: false,
                touch_pending: false,
                deciphers: 0,
                authentications: 0,
                touches: 0,
                chain: Vec::new(),
                present: true,
                plugged: 0,
                reselect_drops_pin: false,
                policies: [
                    SimTouch::from_flag(options.decrypt_touch),
                    SimTouch::from_flag(options.auth_touch),
                ],
                last_touch: [None, None],
                touch_cache: TOUCH_CACHE,
                waiting_for_touch: false,
                time_out_now: false,
                verify_attempts: 0,
                pin_queries: 0,
                kdf: None,
            }),
            options,
            touched: Condvar::new(),
        })
    }

    /// How many authentication signatures the key has made. With the touch
    /// policy on, this is also how many touches SSH has cost.
    pub fn authentication_count(&self) -> u32 {
        self.lock().authentications
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Tests share the key between threads; one that panicked while
        // holding the lock has already failed its test.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn uif(&self, slot: Slot) -> Vec<u8> {
        vec![self.lock().policies[slot as usize].uif(), 0x20]
    }

    /// [`CardConnection::transmit`]: fails while unplugged, and when the key
    /// was unplugged while the command ran.
    fn transmit_checked(&self, apdu: &[u8]) -> Result<Vec<u8>, String> {
        let plugged = {
            let state = self.lock();
            if !state.present {
                return Err("the key is not connected".into());
            }
            state.plugged
        };
        let (mut body, sw) = match self.handle(apdu) {
            Ok(body) => (body, apdu::SW_OK),
            Err(sw) => (vec![], sw),
        };
        let state = self.lock();
        if !state.present || state.plugged != plugged {
            return Err("the key was removed".into());
        }
        body.extend(sw.to_be_bytes());
        Ok(body)
    }

    /// The public half of the decryption key, as a session reads it.
    pub fn decryption_key(self: &Arc<Self>) -> Result<pgp::packet::PublicKey, crate::PawError> {
        crate::key::card::card_decryption_key(self.clone()).map_err(|e| crate::PawError::Store(e.to_string()))
    }

    pub fn authentication_public_key(&self) -> [u8; 32] {
        self.auth_key.verifying_key().to_bytes()
    }

    /// Hex OpenPGP v4 fingerprint of the authentication key.
    pub fn authentication_fingerprint_hex(&self) -> String {
        crate::fingerprint::hex(&self.auth_fingerprint())
    }

    fn public(&self) -> [u8; 32] {
        PublicKey::from(&self.secret).to_bytes()
    }

    /// Fingerprint of the ECDH key with SHA-256 KDF and AES-128 key wrap.
    fn fingerprint(&self) -> [u8; 20] {
        fingerprint_v4(18, &OID_CV25519, self.public(), &[3, 1, 8, 7])
    }

    fn auth_fingerprint(&self) -> [u8; 20] {
        fingerprint_v4(22, &OID_ED25519, self.authentication_public_key(), &[])
    }

    fn pw_status(&self) -> Vec<u8> {
        let tries = self.lock().tries_left;
        vec![0x00, 0x7F, 0x7F, 0x7F, tries, 0x00, 0x03]
    }

    fn fingerprints(&self) -> Vec<u8> {
        let mut fps = vec![0u8; 20];
        fps.extend(self.fingerprint());
        fps.extend(self.auth_fingerprint());
        fps
    }

    fn generation_times(&self) -> Vec<u8> {
        let mut times = vec![0u8; 4];
        times.extend(CREATED.to_be_bytes());
        times.extend(CREATED.to_be_bytes());
        times
    }

    fn application_related_data(&self) -> Vec<u8> {
        let mut sig_attrs = vec![22];
        sig_attrs.extend(OID_ED25519);
        let mut dec_attrs = vec![18];
        dec_attrs.extend(OID_CV25519);

        let mut discretionary = Vec::new();
        // Extended capabilities: no secure messaging, no KDF.
        discretionary.extend(tlv(
            &[0xC0],
            &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00],
        ));
        discretionary.extend(tlv(&[0xC1], &sig_attrs));
        discretionary.extend(tlv(&[0xC2], &dec_attrs));
        discretionary.extend(tlv(&[0xC3], &sig_attrs));
        discretionary.extend(tlv(&[0xC4], &self.pw_status()));
        discretionary.extend(tlv(&[0xC5], &self.fingerprints()));
        discretionary.extend(tlv(&[0xC6], &[0u8; 60]));
        discretionary.extend(tlv(&[0xCD], &self.generation_times()));
        discretionary.extend(tlv(&[0xD6], &[0x00, 0x20]));
        discretionary.extend(tlv(&[0xD7], &self.uif(Slot::Decryption)));
        discretionary.extend(tlv(&[0xD8], &self.uif(Slot::Authentication)));

        let [s0, s1, s2, s3] = self.serial;
        let aid = [0xD2, 0x76, 0x00, 0x01, 0x24, 0x01, 0x03, 0x04, 0x00, 0x06, s0, s1, s2, s3, 0x00, 0x00];
        let mut ard = tlv(&[0x4F], &aid);
        // Historical bytes: command chaining (0x80), and extended Lc and Le
        // (0x40) if asked for.
        let capabilities = if self.options.extended_length { 0xC0 } else { 0x80 };
        ard.extend(tlv(
            &[0x5F, 0x52],
            &[0x00, 0x73, 0x00, 0x00, capabilities, 0x05, 0x90, 0x00],
        ));
        if self.options.extended_length {
            let [hi, lo] = MAX_EXTENDED_COMMAND.to_be_bytes();
            ard.extend(tlv(&[0x7F, 0x66], &[0x02, 0x02, hi, lo, 0x02, 0x02, hi, lo]));
        }
        ard.extend(tlv(&[0x73], &discretionary));
        tlv(&[0x6E], &ard)
    }

    fn handle(&self, apdu: &[u8]) -> Result<Vec<u8>, u16> {
        if apdu.len() < 4 {
            return Err(apdu::SW_WRONG_LENGTH);
        }
        let (cla, ins, p1, p2) = (apdu[0], apdu[1], apdu[2], apdu[3]);
        let part = command_data(&apdu[4..]).ok_or(apdu::SW_WRONG_LENGTH)?;

        // Command chaining: every part but the last has CLA bit 0x10 set and
        // is only stored.
        let data = {
            let mut state = self.lock();
            state.chain.extend_from_slice(part);
            if cla & 0x10 != 0 {
                return Ok(vec![]);
            }
            std::mem::take(&mut state.chain)
        };
        let data = data.as_slice();

        match (ins, p1, p2) {
            (SELECT, 0x04, _) => {
                let mut state = self.lock();
                if state.reselect_drops_pin {
                    state.verified = false;
                }
                Ok(vec![])
            }
            (GET_DATA, 0x00, 0x6E) => Ok(self.application_related_data()),
            (GET_DATA, 0x00, apdu::PW_STATUS) => Ok(self.pw_status()),
            (GET_DATA, 0x00, 0xC5) => Ok(self.fingerprints()),
            (GET_DATA, 0x00, 0xCD) => Ok(self.generation_times()),
            (GET_DATA, 0x00, 0xD7) => Ok(self.uif(Slot::Decryption)),
            (GET_DATA, 0x00, 0xD8) => Ok(self.uif(Slot::Authentication)),
            (GET_DATA, 0x00, apdu::KDF) => self.lock().kdf.clone().ok_or(apdu::SW_DATA_NOT_FOUND),
            (GET_DATA, _, _) => Err(apdu::SW_DATA_NOT_FOUND),
            (VERIFY, VERIFY_P1, PW1_82) => {
                let mut state = self.lock();
                if data.is_empty() {
                    state.pin_queries += 1;
                } else {
                    state.verify_attempts += 1;
                }
                if state.tries_left == 0 {
                    return Err(apdu::SW_PIN_BLOCKED);
                }
                if data.is_empty() {
                    return if state.verified { Ok(vec![]) } else { Err(apdu::SW_TRIES_LEFT | u16::from(state.tries_left)) };
                }
                let expected = match &state.kdf {
                    Some(object) => crate::key::kdf::PinKdf::parse(object)
                        .derive(state.pin.as_bytes())
                        .map_err(|_| apdu::SW_NO_DIAGNOSIS)?,
                    None => zeroize::Zeroizing::new(state.pin.as_bytes().to_vec()),
                };
                if data == expected.as_slice() {
                    state.tries_left = 3;
                    state.verified = true;
                    Ok(vec![])
                } else {
                    state.tries_left -= 1;
                    state.verified = false;
                    Err(apdu::SW_SECURITY_NOT_SATISFIED)
                }
            }
            (VERIFY, FORGET_P1, PW1_82) => {
                self.lock().verified = false;
                Ok(vec![])
            }
            (0x47, 0x81, 0x00) if data == [0xB8, 0x00] => {
                Ok(tlv(&[0x7F, 0x49], &tlv(&[0x86], &self.public())))
            }
            (0x47, 0x81, 0x00) if data == [0xA4, 0x00] => Ok(tlv(
                &[0x7F, 0x49],
                &tlv(&[0x86], &self.authentication_public_key()),
            )),
            (PSO, DECIPHER_P1, DECIPHER_P2) => self.decipher(data),
            (INTERNAL_AUTHENTICATE, 0x00, 0x00) => self.internal_authenticate(data),
            _ => Err(apdu::SW_INS_NOT_SUPPORTED),
        }
    }

    /// A touch that does not come in time fails with 6982, as does a
    /// removal. What a YubiKey answers after its touch timeout is not yet
    /// known.
    fn authorise(&self, slot: Slot) -> Result<MutexGuard<'_, State>, u16> {
        let mut state = self.lock();
        if !state.verified {
            return Err(apdu::SW_SECURITY_NOT_SATISFIED);
        }
        let cached = state.last_touch[slot as usize]
            .is_some_and(|at| at.elapsed() < state.touch_cache);
        let wait = match state.policies[slot as usize] {
            SimTouch::Off => false,
            SimTouch::EachUse => true,
            SimTouch::Cached => !cached,
        };
        if wait {
            let waiting_since = Instant::now();
            state.waiting_for_touch = true;
            let (guard, _) = self
                .touched
                .wait_timeout_while(state, self.options.touch_timeout, |s| {
                    !s.touch_pending && s.present && !s.time_out_now
                })
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = guard;
            state.waiting_for_touch = false;
            if std::mem::take(&mut state.time_out_now) {
                // A real key gives up after about 15 seconds; the core
                // reads a refusal quicker than a second as something else.
                // So a timeout asked for at once still takes that second.
                drop(state);
                std::thread::sleep(MIN_TOUCH_TIMEOUT.saturating_sub(waiting_since.elapsed()));
                return Err(apdu::SW_SECURITY_NOT_SATISFIED);
            }
            if !state.touch_pending || !state.present {
                return Err(apdu::SW_SECURITY_NOT_SATISFIED);
            }
            state.touch_pending = false;
            state.touches += 1;
            state.last_touch[slot as usize] = Some(Instant::now());
        }
        Ok(state)
    }

    /// INTERNAL AUTHENTICATE with an ed25519 key signs the data as given
    /// (PureEdDSA): no hashing or padding by the caller.
    fn internal_authenticate(&self, data: &[u8]) -> Result<Vec<u8>, u16> {
        if data.is_empty() {
            return Err(apdu::SW_WRONG_DATA);
        }
        let mut state = self.authorise(Slot::Authentication)?;
        state.authentications += 1;
        Ok(self.auth_key.sign(data).to_bytes().to_vec())
    }

    fn decipher(&self, data: &[u8]) -> Result<Vec<u8>, u16> {
        // A6 { 7F49 { 86 { point } } }: the point is the last 32 bytes.
        if data.len() < 32 || data[0] != 0xA6 {
            return Err(apdu::SW_WRONG_DATA);
        }
        let mut state = self.authorise(Slot::Decryption)?;
        let mut point = [0u8; 32];
        point.copy_from_slice(&data[data.len() - 32..]);
        state.deciphers += 1;
        Ok(self
            .secret
            .diffie_hellman(&PublicKey::from(point))
            .to_bytes()
            .to_vec())
    }
}


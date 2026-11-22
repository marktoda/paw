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

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use sha1::{Digest, Sha1};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::ApduTransport;

const CREATED: u32 = 1_780_000_000;
const OID_CV25519: [u8; 10] = [0x2B, 0x06, 0x01, 0x04, 0x01, 0x97, 0x55, 0x01, 0x05, 0x01];
const OID_ED25519: [u8; 9] = [0x2B, 0x06, 0x01, 0x04, 0x01, 0xDA, 0x47, 0x0F, 0x01];

/// Largest command the card claims to accept when it advertises extended
/// length. A YubiKey 5 reports a value of this order.
const MAX_EXTENDED_COMMAND: u16 = 2048;

/// How a simulated key is set up. [`SimulatedKey::new`] covers the common
/// case; tests that need the slots to differ build this directly.
pub struct SimOptions {
    pub pin: String,
    /// Touch policy of the decryption slot.
    pub decrypt_touch: bool,
    /// Touch policy of the authentication slot. A real key sets it per slot.
    pub auth_touch: bool,
    /// How long a key operation waits for [`SimulatedKey::touch`] before
    /// failing, as a real key does when nobody touches it.
    pub touch_timeout: Duration,
    /// Advertise extended-length APDUs as well as command chaining.
    pub extended_length: bool,
}

#[derive(uniffi::Object)]
pub struct SimulatedKey {
    secret: StaticSecret,
    auth_key: SigningKey,
    options: SimOptions,
    state: Mutex<State>,
    touched: Condvar,
}

struct State {
    tries_left: u8,
    verified: bool,
    touch_pending: bool,
    deciphers: u32,
    authentications: u32,
    /// Data of a chained command received so far (CLA bit 0x10).
    chain: Vec<u8>,
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
    /// `touch_timeout_ms` is how long a decrypt waits for [`Self::touch`]
    /// before failing, as a real key does when nobody touches it.
    #[uniffi::constructor]
    pub fn new(pin: String, touch_required: bool, touch_timeout_ms: u32) -> Arc<Self> {
        Self::with_options(SimOptions {
            pin,
            decrypt_touch: touch_required,
            auth_touch: touch_required,
            touch_timeout: Duration::from_millis(touch_timeout_ms.into()),
            extended_length: false,
        })
    }

    /// Stands in for a finger on the gold disc. One touch allows one key
    /// operation (a decrypt or an authentication signature).
    pub fn touch(&self) {
        self.state.lock().unwrap().touch_pending = true;
        self.touched.notify_all();
    }

    /// How many decrypts the key has performed.
    pub fn decipher_count(&self) -> u32 {
        self.state.lock().unwrap().deciphers
    }

    /// Hex OpenPGP v4 fingerprint of the decryption key.
    pub fn fingerprint_hex(&self) -> String {
        self.fingerprint().iter().map(|b| format!("{b:02X}")).collect()
    }

    /// One command APDU in, one response APDU (with status bytes) out.
    pub fn exchange(&self, apdu: Vec<u8>) -> Vec<u8> {
        let (mut body, sw) = match self.handle(&apdu) {
            Ok(body) => (body, 0x9000u16),
            Err(sw) => (vec![], sw),
        };
        body.extend(sw.to_be_bytes());
        body
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
        Arc::new(Self {
            secret: StaticSecret::from([7u8; 32]),
            auth_key: SigningKey::from_bytes(&[9u8; 32]),
            options,
            state: Mutex::new(State {
                tries_left: 3,
                verified: false,
                touch_pending: false,
                deciphers: 0,
                authentications: 0,
                chain: Vec::new(),
            }),
            touched: Condvar::new(),
        })
    }

    /// How many authentication signatures the key has made. With the touch
    /// policy on, this is also how many touches SSH has cost.
    pub fn authentication_count(&self) -> u32 {
        self.state.lock().unwrap().authentications
    }

    /// The raw ed25519 public key of the authentication slot.
    pub fn authentication_public_key(&self) -> [u8; 32] {
        self.auth_key.verifying_key().to_bytes()
    }

    /// Hex OpenPGP v4 fingerprint of the authentication key.
    pub fn authentication_fingerprint_hex(&self) -> String {
        self.auth_fingerprint().iter().map(|b| format!("{b:02X}")).collect()
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
        let tries = self.state.lock().unwrap().tries_left;
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
        discretionary.extend(tlv(&[0xD7], &[self.options.decrypt_touch.into(), 0x20]));
        discretionary.extend(tlv(&[0xD8], &[self.options.auth_touch.into(), 0x20]));

        let aid = [
            0xD2, 0x76, 0x00, 0x01, 0x24, 0x01, 0x03, 0x04, 0x00, 0x06, 0x12, 0x34, 0x56, 0x78,
            0x00, 0x00,
        ];
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
            return Err(0x6700);
        }
        let (cla, ins, p1, p2) = (apdu[0], apdu[1], apdu[2], apdu[3]);
        let part = command_data(&apdu[4..]).ok_or(0x6700u16)?;

        // Command chaining: every part but the last has CLA bit 0x10 set and
        // is only stored.
        let data = {
            let mut state = self.state.lock().unwrap();
            state.chain.extend_from_slice(part);
            if cla & 0x10 != 0 {
                return Ok(vec![]);
            }
            std::mem::take(&mut state.chain)
        };
        let data = data.as_slice();

        match (ins, p1, p2) {
            (0xA4, 0x04, _) => Ok(vec![]),
            (0xCA, 0x00, 0x6E) => Ok(self.application_related_data()),
            (0xCA, 0x00, 0xC4) => Ok(self.pw_status()),
            (0xCA, 0x00, 0xC5) => Ok(self.fingerprints()),
            (0xCA, 0x00, 0xCD) => Ok(self.generation_times()),
            (0xCA, 0x00, 0xD7) => Ok(vec![self.options.decrypt_touch.into(), 0x20]),
            (0xCA, 0x00, 0xD8) => Ok(vec![self.options.auth_touch.into(), 0x20]),
            (0xCA, _, _) => Err(0x6A88),
            (0x20, 0x00, 0x82) => {
                let mut state = self.state.lock().unwrap();
                if state.tries_left == 0 {
                    return Err(0x6983);
                }
                if data == self.options.pin.as_bytes() {
                    state.tries_left = 3;
                    state.verified = true;
                    Ok(vec![])
                } else {
                    state.tries_left -= 1;
                    state.verified = false;
                    Err(0x6982)
                }
            }
            (0x47, 0x81, 0x00) if data == [0xB8, 0x00] => {
                Ok(tlv(&[0x7F, 0x49], &tlv(&[0x86], &self.public())))
            }
            (0x47, 0x81, 0x00) if data == [0xA4, 0x00] => Ok(tlv(
                &[0x7F, 0x49],
                &tlv(&[0x86], &self.authentication_public_key()),
            )),
            (0x2A, 0x80, 0x86) => self.decipher(data),
            (0x88, 0x00, 0x00) => self.internal_authenticate(data),
            _ => Err(0x6D00),
        }
    }

    /// Checks that the PIN was verified and, if the slot's policy asks for
    /// it, waits for a touch and consumes it.
    fn authorise(&self, touch_required: bool) -> Result<MutexGuard<'_, State>, u16> {
        let mut state = self.state.lock().unwrap();
        if !state.verified {
            return Err(0x6982);
        }
        if touch_required {
            let (guard, _) = self
                .touched
                .wait_timeout_while(state, self.options.touch_timeout, |s| !s.touch_pending)
                .unwrap();
            state = guard;
            if !state.touch_pending {
                return Err(0x6982);
            }
            state.touch_pending = false;
        }
        Ok(state)
    }

    /// INTERNAL AUTHENTICATE with an ed25519 key signs the data as given
    /// (PureEdDSA): no hashing or padding by the caller.
    fn internal_authenticate(&self, data: &[u8]) -> Result<Vec<u8>, u16> {
        if data.is_empty() {
            return Err(0x6A80);
        }
        let mut state = self.authorise(self.options.auth_touch)?;
        state.authentications += 1;
        Ok(self.auth_key.sign(data).to_bytes().to_vec())
    }

    fn decipher(&self, data: &[u8]) -> Result<Vec<u8>, u16> {
        // A6 { 7F49 { 86 { point } } }: the point is the last 32 bytes.
        if data.len() < 32 || data[0] != 0xA6 {
            return Err(0x6A80);
        }
        let mut state = self.authorise(self.options.decrypt_touch)?;
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

impl ApduTransport for SimulatedKey {
    fn transmit(&self, apdu: &[u8]) -> Result<Vec<u8>, String> {
        Ok(self.exchange(apdu.to_vec()))
    }
}

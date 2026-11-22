//! A simulated OpenPGP card, for tests and for running the app in the iOS
//! simulator, where no real key can be attached.
//!
//! It implements the minimum of OpenPGP card 3.4 needed to read status,
//! verify the user PIN and run ECDH with a Curve25519 key, using short APDUs
//! only. Its private key is a fixed, public test value: it protects nothing.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use sha1::{Digest, Sha1};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::ApduTransport;

const CREATED: u32 = 1_780_000_000;
const OID_CV25519: [u8; 10] = [0x2B, 0x06, 0x01, 0x04, 0x01, 0x97, 0x55, 0x01, 0x05, 0x01];
const OID_ED25519: [u8; 9] = [0x2B, 0x06, 0x01, 0x04, 0x01, 0xDA, 0x47, 0x0F, 0x01];

#[derive(uniffi::Object)]
pub struct SimulatedKey {
    secret: StaticSecret,
    pin: String,
    touch_required: bool,
    touch_timeout: Duration,
    state: Mutex<State>,
    touched: Condvar,
}

struct State {
    tries_left: u8,
    verified: bool,
    touch_pending: bool,
    deciphers: u32,
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
        Arc::new(Self {
            secret: StaticSecret::from([7u8; 32]),
            pin,
            touch_required,
            touch_timeout: Duration::from_millis(touch_timeout_ms.into()),
            state: Mutex::new(State {
                tries_left: 3,
                verified: false,
                touch_pending: false,
                deciphers: 0,
            }),
            touched: Condvar::new(),
        })
    }

    /// Stands in for a finger on the gold disc. One touch allows one decrypt.
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

impl SimulatedKey {
    fn public(&self) -> [u8; 32] {
        PublicKey::from(&self.secret).to_bytes()
    }

    /// Fingerprint of the ECDH key with SHA-256 KDF and AES-128 key wrap.
    fn fingerprint(&self) -> [u8; 20] {
        let mut body = vec![4];
        body.extend(CREATED.to_be_bytes());
        body.push(18);
        body.push(OID_CV25519.len() as u8);
        body.extend(OID_CV25519);
        body.extend(263u16.to_be_bytes());
        body.push(0x40);
        body.extend(self.public());
        body.extend([3, 1, 8, 7]);

        let mut hasher = Sha1::new();
        hasher.update([0x99]);
        hasher.update((body.len() as u16).to_be_bytes());
        hasher.update(&body);
        hasher.finalize().into()
    }

    fn touch_policy(&self) -> u8 {
        self.touch_required.into()
    }

    fn pw_status(&self) -> Vec<u8> {
        let tries = self.state.lock().unwrap().tries_left;
        vec![0x00, 0x7F, 0x7F, 0x7F, tries, 0x00, 0x03]
    }

    fn fingerprints(&self) -> Vec<u8> {
        let mut fps = vec![0u8; 20];
        fps.extend(self.fingerprint());
        fps.extend([0u8; 20]);
        fps
    }

    fn generation_times(&self) -> Vec<u8> {
        let mut times = vec![0u8; 4];
        times.extend(CREATED.to_be_bytes());
        times.extend([0u8; 4]);
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
        discretionary.extend(tlv(&[0xD7], &[self.touch_policy(), 0x20]));
        discretionary.extend(tlv(&[0xD8], &[0x00, 0x20]));

        let aid = [
            0xD2, 0x76, 0x00, 0x01, 0x24, 0x01, 0x03, 0x04, 0x00, 0x06, 0x12, 0x34, 0x56, 0x78,
            0x00, 0x00,
        ];
        let mut ard = tlv(&[0x4F], &aid);
        // Historical bytes: command chaining supported, short APDUs only.
        ard.extend(tlv(
            &[0x5F, 0x52],
            &[0x00, 0x73, 0x00, 0x00, 0x80, 0x05, 0x90, 0x00],
        ));
        ard.extend(tlv(&[0x73], &discretionary));
        tlv(&[0x6E], &ard)
    }

    fn handle(&self, apdu: &[u8]) -> Result<Vec<u8>, u16> {
        if apdu.len() < 4 {
            return Err(0x6700);
        }
        let (ins, p1, p2) = (apdu[1], apdu[2], apdu[3]);
        let data = match apdu.get(4).map(|lc| *lc as usize) {
            Some(lc) if apdu.len() >= 5 + lc && apdu.len() > 5 => &apdu[5..5 + lc],
            _ => &[][..],
        };

        match (ins, p1, p2) {
            (0xA4, 0x04, _) => Ok(vec![]),
            (0xCA, 0x00, 0x6E) => Ok(self.application_related_data()),
            (0xCA, 0x00, 0xC4) => Ok(self.pw_status()),
            (0xCA, 0x00, 0xC5) => Ok(self.fingerprints()),
            (0xCA, 0x00, 0xCD) => Ok(self.generation_times()),
            (0xCA, 0x00, 0xD7) => Ok(vec![self.touch_policy(), 0x20]),
            (0xCA, _, _) => Err(0x6A88),
            (0x20, 0x00, 0x82) => {
                let mut state = self.state.lock().unwrap();
                if state.tries_left == 0 {
                    return Err(0x6983);
                }
                if data == self.pin.as_bytes() {
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
            (0x2A, 0x80, 0x86) => self.decipher(data),
            _ => Err(0x6D00),
        }
    }

    fn decipher(&self, data: &[u8]) -> Result<Vec<u8>, u16> {
        let mut state = self.state.lock().unwrap();
        if !state.verified {
            return Err(0x6982);
        }
        // A6 { 7F49 { 86 { point } } }: the point is the last 32 bytes.
        if data.len() < 32 || data[0] != 0xA6 {
            return Err(0x6A80);
        }
        if self.touch_required {
            let (guard, _) = self
                .touched
                .wait_timeout_while(state, self.touch_timeout, |s| !s.touch_pending)
                .unwrap();
            state = guard;
            if !state.touch_pending {
                return Err(0x6982);
            }
            state.touch_pending = false;
        }
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

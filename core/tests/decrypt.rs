//! End-to-end decrypt against a simulated OpenPGP card holding a Curve25519
//! decryption key, the same key type as on a current YubiKey.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use paw_core::{card_info, decrypt_entry, ApduTransport, Error};
use sha1::{Digest, Sha1};
use x25519_dalek::{PublicKey, StaticSecret};

const PIN: &str = "123456";
const CREATED: u32 = 1_780_000_000;
const OID_CV25519: [u8; 10] = [0x2B, 0x06, 0x01, 0x04, 0x01, 0x97, 0x55, 0x01, 0x05, 0x01];
const OID_ED25519: [u8; 9] = [0x2B, 0x06, 0x01, 0x04, 0x01, 0xDA, 0x47, 0x0F, 0x01];

/// The minimum of an OpenPGP card 3.4 needed to read its status, verify the
/// user PIN and run ECDH with a Curve25519 key. Short APDUs only.
struct FakeCard {
    secret: StaticSecret,
    touch_policy: u8,
    state: Mutex<State>,
    deciphers: AtomicU32,
}

struct State {
    tries_left: u8,
    verified: bool,
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

impl FakeCard {
    fn new(touch_policy: u8) -> Self {
        Self {
            secret: StaticSecret::from([7u8; 32]),
            touch_policy,
            state: Mutex::new(State {
                tries_left: 3,
                verified: false,
            }),
            deciphers: AtomicU32::new(0),
        }
    }

    fn public(&self) -> [u8; 32] {
        PublicKey::from(&self.secret).to_bytes()
    }

    /// OpenPGP v4 fingerprint of the ECDH key (SHA-256 KDF, AES-128 wrap).
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
        // Extended capabilities: no secure messaging, no KDF, button present.
        discretionary.extend(tlv(&[0xC0], &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00]));
        discretionary.extend(tlv(&[0xC1], &sig_attrs));
        discretionary.extend(tlv(&[0xC2], &dec_attrs));
        discretionary.extend(tlv(&[0xC3], &sig_attrs));
        discretionary.extend(tlv(&[0xC4], &self.pw_status()));
        discretionary.extend(tlv(&[0xC5], &self.fingerprints()));
        discretionary.extend(tlv(&[0xC6], &[0u8; 60]));
        discretionary.extend(tlv(&[0xCD], &self.generation_times()));
        discretionary.extend(tlv(&[0xD6], &[0x00, 0x20]));
        discretionary.extend(tlv(&[0xD7], &[self.touch_policy, 0x20]));
        discretionary.extend(tlv(&[0xD8], &[0x00, 0x20]));

        let aid = [
            0xD2, 0x76, 0x00, 0x01, 0x24, 0x01, 0x03, 0x04, 0x00, 0x06, 0x12, 0x34, 0x56, 0x78,
            0x00, 0x00,
        ];
        let mut ard = tlv(&[0x4F], &aid);
        // Historical bytes: command chaining supported, short APDUs only.
        ard.extend(tlv(&[0x5F, 0x52], &[0x00, 0x73, 0x00, 0x00, 0x80, 0x05, 0x90, 0x00]));
        ard.extend(tlv(&[0x73], &discretionary));
        tlv(&[0x6E], &ard)
    }

    fn handle(&self, apdu: &[u8]) -> Result<Vec<u8>, u16> {
        let (ins, p1, p2) = (apdu[1], apdu[2], apdu[3]);
        let data = if apdu.len() > 5 {
            let lc = apdu[4] as usize;
            &apdu[5..5 + lc]
        } else {
            &[][..]
        };

        match (ins, p1, p2) {
            (0xA4, 0x04, _) => Ok(vec![]),
            (0xCA, 0x00, 0x6E) => Ok(self.application_related_data()),
            (0xCA, 0x00, 0xC4) => Ok(self.pw_status()),
            (0xCA, 0x00, 0xC5) => Ok(self.fingerprints()),
            (0xCA, 0x00, 0xCD) => Ok(self.generation_times()),
            (0xCA, 0x00, 0xD7) => Ok(vec![self.touch_policy, 0x20]),
            (0xCA, _, _) => Err(0x6A88),
            (0x20, 0x00, 0x82) => {
                let mut state = self.state.lock().unwrap();
                if state.tries_left == 0 {
                    return Err(0x6983);
                }
                if data == PIN.as_bytes() {
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
            (0x2A, 0x80, 0x86) => {
                if !self.state.lock().unwrap().verified {
                    return Err(0x6982);
                }
                // A6 { 7F49 { 86 { point } } }: the point is the last 32 bytes.
                if data.len() < 32 || data[0] != 0xA6 {
                    return Err(0x6A80);
                }
                let mut point = [0u8; 32];
                point.copy_from_slice(&data[data.len() - 32..]);
                self.deciphers.fetch_add(1, Ordering::SeqCst);
                Ok(self
                    .secret
                    .diffie_hellman(&PublicKey::from(point))
                    .to_bytes()
                    .to_vec())
            }
            _ => Err(0x6D00),
        }
    }
}

impl ApduTransport for FakeCard {
    fn transmit(&self, apdu: &[u8]) -> Result<Vec<u8>, String> {
        let (mut body, sw) = match self.handle(apdu) {
            Ok(body) => (body, 0x9000u16),
            Err(sw) => (vec![], sw),
        };
        body.extend(sw.to_be_bytes());
        Ok(body)
    }
}

fn encrypt_for(card: &Arc<FakeCard>, plaintext: &str) -> Vec<u8> {
    paw_core::encrypt_to_card(card.clone(), plaintext.as_bytes()).unwrap()
}

const ENTRY: &str = "correct horse battery staple\nlogin: you@example.com\nurl: example.com\n";

#[test]
fn reads_card_status_without_a_pin() {
    let card = Arc::new(FakeCard::new(0x01));
    let info = card_info(card.clone()).unwrap();

    assert_eq!(info.ident, "0006:12345678");
    assert_eq!(
        info.decryption_fingerprint.as_deref().map(str::to_lowercase),
        Some(card.fingerprint().iter().map(|b| format!("{b:02x}")).collect())
    );
    assert_eq!(info.authentication_fingerprint, None);
    assert_eq!(info.pin_tries_left, 3);
    assert!(info.touch_required);
}

#[test]
fn decrypts_an_entry_and_announces_the_touch() {
    let card = Arc::new(FakeCard::new(0x01));
    let ciphertext = encrypt_for(&card, ENTRY);

    let touched = AtomicBool::new(false);
    let plaintext = decrypt_entry(card.clone(), PIN, &ciphertext, &|| {
        touched.store(true, Ordering::SeqCst)
    })
    .unwrap();

    assert_eq!(String::from_utf8(plaintext).unwrap(), ENTRY);
    assert!(touched.load(Ordering::SeqCst));
    assert_eq!(card.deciphers.load(Ordering::SeqCst), 1);
}

#[test]
fn no_touch_prompt_when_the_policy_is_off() {
    let card = Arc::new(FakeCard::new(0x00));
    let ciphertext = encrypt_for(&card, ENTRY);

    let touched = AtomicBool::new(false);
    decrypt_entry(card, PIN, &ciphertext, &|| touched.store(true, Ordering::SeqCst)).unwrap();

    assert!(!touched.load(Ordering::SeqCst));
}

#[test]
fn wrong_pin_reports_tries_left_then_blocks() {
    let card = Arc::new(FakeCard::new(0x01));
    let ciphertext = encrypt_for(&card, ENTRY);

    for expected in [2, 1] {
        match decrypt_entry(card.clone(), "000000", &ciphertext, &|| {}) {
            Err(Error::WrongPin { tries_left }) => assert_eq!(tries_left, expected),
            other => panic!("expected WrongPin, got {other:?}"),
        }
    }
    assert!(matches!(
        decrypt_entry(card.clone(), "000000", &ciphertext, &|| {}),
        Err(Error::PinBlocked)
    ));
    // Even the right PIN is refused once blocked, and nothing was deciphered.
    assert!(matches!(
        decrypt_entry(card.clone(), PIN, &ciphertext, &|| {}),
        Err(Error::PinBlocked)
    ));
    assert_eq!(card.deciphers.load(Ordering::SeqCst), 0);
}

#[test]
fn rejects_bytes_that_are_not_an_entry() {
    let card = Arc::new(FakeCard::new(0x01));
    assert!(matches!(
        decrypt_entry(card, PIN, b"not a gpg file", &|| {}),
        Err(Error::NotAnEntry(_))
    ));
}

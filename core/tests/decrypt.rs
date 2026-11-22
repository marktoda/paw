//! End-to-end decrypt against a simulated OpenPGP card holding a Curve25519
//! decryption key, the same key type as on a current YubiKey.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use paw_core::sim::SimulatedKey;
use paw_core::{card_info, decrypt_entry, encrypt_to_card, PawError};

const PIN: &str = "123456";
const ENTRY: &str = "correct horse battery staple\nlogin: you@example.com\nurl: example.com\n";

fn key(touch_required: bool) -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), touch_required, 50)
}

fn entry_for(key: &Arc<SimulatedKey>) -> Vec<u8> {
    encrypt_to_card(key.clone(), ENTRY.as_bytes()).unwrap()
}

#[test]
fn reads_card_status_without_a_pin() {
    let key = key(true);
    let info = card_info(key.clone()).unwrap();

    assert_eq!(info.ident, "0006:12345678");
    assert_eq!(info.decryption_fingerprint, Some(key.fingerprint_hex()));
    assert_eq!(
        info.authentication_fingerprint,
        Some(key.authentication_fingerprint_hex())
    );
    assert_eq!(info.pin_tries_left, 3);
    assert!(info.touch_required);
}

#[test]
fn decrypts_an_entry_once_the_key_is_touched() {
    let key = key(true);
    let ciphertext = entry_for(&key);

    let asked = AtomicBool::new(false);
    let plaintext = decrypt_entry(key.clone(), PIN, &ciphertext, &|| {
        asked.store(true, Ordering::SeqCst);
        key.touch();
    })
    .unwrap();

    assert_eq!(String::from_utf8(plaintext).unwrap(), ENTRY);
    assert!(asked.load(Ordering::SeqCst));
    assert_eq!(key.decipher_count(), 1);
}

#[test]
fn fails_when_nobody_touches_the_key() {
    let key = key(true);
    let ciphertext = entry_for(&key);

    assert!(matches!(
        decrypt_entry(key.clone(), PIN, &ciphertext, &|| {}),
        Err(PawError::Decrypt(_))
    ));
    assert_eq!(key.decipher_count(), 0);
}

#[test]
fn no_touch_prompt_when_the_policy_is_off() {
    let key = key(false);
    let ciphertext = entry_for(&key);

    let asked = AtomicBool::new(false);
    decrypt_entry(key, PIN, &ciphertext, &|| asked.store(true, Ordering::SeqCst)).unwrap();

    assert!(!asked.load(Ordering::SeqCst));
}

#[test]
fn wrong_pin_reports_tries_left_then_blocks() {
    let key = key(true);
    let ciphertext = entry_for(&key);

    for expected in [2, 1] {
        match decrypt_entry(key.clone(), "000000", &ciphertext, &|| key.touch()) {
            Err(PawError::WrongPin { tries_left }) => assert_eq!(tries_left, expected),
            other => panic!("expected WrongPin, got {other:?}"),
        }
    }
    assert!(matches!(
        decrypt_entry(key.clone(), "000000", &ciphertext, &|| key.touch()),
        Err(PawError::PinBlocked)
    ));
    // Even the right PIN is refused once blocked, and nothing was deciphered.
    assert!(matches!(
        decrypt_entry(key.clone(), PIN, &ciphertext, &|| key.touch()),
        Err(PawError::PinBlocked)
    ));
    assert_eq!(key.decipher_count(), 0);
}

#[test]
fn a_correct_pin_resets_the_tries() {
    let key = key(false);
    let ciphertext = entry_for(&key);

    assert!(decrypt_entry(key.clone(), "000000", &ciphertext, &|| {}).is_err());
    decrypt_entry(key.clone(), PIN, &ciphertext, &|| {}).unwrap();

    assert_eq!(card_info(key).unwrap().pin_tries_left, 3);
}

#[test]
fn rejects_bytes_that_are_not_an_entry() {
    assert!(matches!(
        decrypt_entry(key(true), PIN, b"not a gpg file", &|| {}),
        Err(PawError::NotAnEntry(_))
    ));
}

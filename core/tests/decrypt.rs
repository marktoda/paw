//! End-to-end decrypt against a simulated OpenPGP card holding a Curve25519
//! decryption key, the same key type as on a current YubiKey.

#![cfg(feature = "sim")]

mod key_common;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use paw_core::sim::SimulatedKey;
use paw_core::{card_info, decrypt_entry, encrypt_to_card, KeyTouch, PawError};

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
    assert_eq!(info.decryption_touch, KeyTouch::EachUse);
    assert_eq!(info.authentication_touch, KeyTouch::EachUse);
    assert_eq!(card_info(self::key(false)).unwrap().decryption_touch, KeyTouch::Off);
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

#[test]
fn an_entry_that_expands_without_end_is_refused() {
    use pgp::composed::MessageBuilder;
    use pgp::crypto::sym::SymmetricKeyAlgorithm;
    use pgp::types::CompressionAlgorithm;

    let key = key(false);
    let public = paw_core::card_decryption_key(key.clone()).unwrap();
    let mut rng = rand::thread_rng();
    // 8 MiB of zeros compresses to a few kilobytes.
    let mut plain = MessageBuilder::from_bytes("", vec![0u8; 8 << 20]);
    plain.compression(CompressionAlgorithm::ZLIB);
    let mut builder = plain.seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    builder.encrypt_to_key(&mut rng, &public).unwrap();
    let ciphertext = builder.to_vec(&mut rng).unwrap();
    assert!(ciphertext.len() < 64 << 10, "the test entry should be small on disk");

    match decrypt_entry(key, PIN, &ciphertext, &|| {}) {
        Err(PawError::Decrypt(why)) => assert!(why.contains("more text"), "{why}"),
        other => panic!("expected a refusal, got {:?}", other.map(|text| text.len())),
    }
}

#[test]
fn an_entry_with_many_packets_for_this_key_costs_one_decipher() {
    let key = key(true);
    let crafted = key_common::with_packet_repeated(&entry_for(&key), 5);
    let asked = AtomicU32::new(0);

    let result = decrypt_entry(key.clone(), PIN, &crafted, &|| {
        asked.fetch_add(1, Ordering::SeqCst);
        key.touch();
    });

    assert!(matches!(result, Err(PawError::Decrypt(_))), "{result:?}");
    assert_eq!(key.decipher_count(), 1);
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

/// The legacy path goes through the openpgp-card library's own KDF code,
/// so this also checks the simulated key's derivation against it.
#[test]
fn the_legacy_path_opens_an_entry_on_a_key_with_a_pin_kdf() {
    let key = key(false);
    key.set_pin_kdf(3);
    let plaintext = decrypt_entry(key.clone(), PIN, &entry_for(&key), &|| {}).unwrap();
    assert_eq!(String::from_utf8(plaintext).unwrap(), ENTRY);
    assert_eq!(key.verify_attempts(), 1);
    assert_eq!(key.pin_tries_left(), 3);
}

//! End-to-end decrypt against a simulated OpenPGP card holding a Curve25519
//! decryption key, the same key type as on a current YubiKey, through a key
//! task as the app runs it.

#![cfg(feature = "sim")]

mod key_common;

use std::sync::Arc;

use key_common::*;
use paw_core::key::{KeyError, KeyTouch};
use paw_core::task::KeyTaskState;
use paw_core::sim::SimulatedKey;
use paw_core::sim::fixture::encrypt;
use paw_core::sim::tasks::decipher;

const TEXT: &str = "correct horse battery staple\nlogin: you@example.com\nurl: example.com\n";

fn key(touch_required: bool) -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), touch_required, 50)
}

fn entry_for(key: &Arc<SimulatedKey>) -> Vec<u8> {
    encrypt(&[key.decryption_key().unwrap()], TEXT.as_bytes()).unwrap()
}

#[test]
fn reads_card_status_without_a_pin() {
    let key = key(true);
    let info = port_with(&key).session().unwrap().info();

    assert_eq!(info.ident, "0006:12345678");
    assert_eq!(info.decryption_fingerprint, paw_core::fingerprint::Fingerprint::parse(&key.fingerprint_hex()));
    assert_eq!(info.authentication_fingerprint, paw_core::fingerprint::Fingerprint::parse(&key.authentication_fingerprint_hex()));
    assert_eq!(info.pin_tries_left, 3);
    assert_eq!(info.decryption_touch, KeyTouch::EachUse);
    assert_eq!(info.authentication_touch, KeyTouch::EachUse);
    assert_eq!(port_with(&self::key(false)).session().unwrap().info().decryption_touch, KeyTouch::Off);
    assert_eq!(key.verify_attempts(), 0);
}

#[test]
fn decrypts_an_entry_once_the_key_is_touched() {
    let key = key(true);
    let ciphertext = entry_for(&key);
    let driver = Driver::helpful(PIN, &key);
    let (task, text) = decipher(&port_with(&key), ciphertext, driver.clone());
    driver.drive(&task);

    assert!(matches!(finish(&task), KeyTaskState::Done { .. }));
    assert_eq!(String::from_utf8(text.recv().unwrap().to_vec()).unwrap(), TEXT);
    assert!(driver.saw(is_touch));
    assert_eq!(key.decipher_count(), 1);
}

#[test]
fn fails_when_nobody_touches_the_key() {
    // Long enough not to read as a refusal at once.
    let key = SimulatedKey::new(PIN.into(), true, 1_100);
    let ciphertext = entry_for(&key);
    let driver = Driver::typist(PIN);
    let (task, _) = decipher(&port_with(&key), ciphertext, driver.clone());
    driver.drive(&task);

    assert!(matches!(finish(&task), KeyTaskState::Failed { error: KeyError::TouchTimeout }));
    assert_eq!(key.decipher_count(), 0);
}

#[test]
fn no_touch_prompt_when_the_policy_is_off() {
    let key = key(false);
    let ciphertext = entry_for(&key);
    let driver = Driver::typist(PIN);
    let (task, _) = decipher(&port_with(&key), ciphertext, driver.clone());
    driver.drive(&task);

    assert!(matches!(finish(&task), KeyTaskState::Done { .. }));
    assert!(!driver.saw(is_touch));
}

#[test]
fn a_correct_pin_resets_the_tries() {
    let key = key(false);
    let ciphertext = entry_for(&key);
    let driver = Driver::new();
    let (task, _) = decipher(&port_with(&key), ciphertext, driver.clone());

    task.supply_pin(b"000000".to_vec());
    driver.wait_for(|s| matches!(s, KeyTaskState::Pin { last_error: Some(_), .. }));
    assert_eq!(key.pin_tries_left(), 2);
    task.supply_pin(PIN.as_bytes().to_vec());

    assert!(matches!(finish(&task), KeyTaskState::Done { .. }));
    assert_eq!(key.pin_tries_left(), 3);
}

#[test]
fn rejects_bytes_that_are_not_an_entry() {
    let error = decrypt(&key(true), PIN, b"not a gpg file").unwrap_err();
    assert!(matches!(error, KeyError::EntryUnreadable { .. }), "{error:?}");
}

#[test]
fn an_entry_that_expands_without_end_is_refused() {
    use pgp::composed::MessageBuilder;
    use pgp::crypto::sym::SymmetricKeyAlgorithm;
    use pgp::types::CompressionAlgorithm;

    let key = key(false);
    let public = key.decryption_key().unwrap();
    let mut rng = rand::thread_rng();
    // 8 MiB of zeros compresses to a few kilobytes.
    let mut plain = MessageBuilder::from_bytes("", vec![0u8; 8 << 20]);
    plain.compression(CompressionAlgorithm::ZLIB);
    let mut builder = plain.seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    builder.encrypt_to_key(&mut rng, &public).unwrap();
    let ciphertext = builder.to_vec(&mut rng).unwrap();
    assert!(ciphertext.len() < 64 << 10, "the test entry should be small on disk");

    match decrypt(&key, PIN, &ciphertext) {
        Err(KeyError::DecryptFailed { detail }) => assert!(detail.contains("more text"), "{detail}"),
        other => panic!("expected a refusal, got {:?}", other.map(|text| text.len())),
    }
}

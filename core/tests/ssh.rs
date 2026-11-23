//! SSH login signatures from a simulated OpenPGP card with an ed25519
//! authentication key, checked with a verifier that shares no code with it.

#![cfg(feature = "sim")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use paw_core::sim::{SimOptions, SimulatedKey};
use paw_core::ssh::{CardSigner, SshPublicKey, SshSigner, card_ssh_public_key, card_ssh_sign};
use paw_core::{PawError, card_info};
use ring::signature::{ED25519, UnparsedPublicKey};

const PIN: &str = "123456";

fn key(decrypt_touch: bool, auth_touch: bool, extended_length: bool) -> Arc<SimulatedKey> {
    SimulatedKey::with_options(SimOptions {
        pin: PIN.into(),
        decrypt_touch,
        auth_touch,
        touch_timeout: Duration::from_millis(50),
        extended_length,
    })
}

/// The raw ed25519 key inside an `ssh-ed25519` blob: its last 32 bytes.
fn raw_key(public: &SshPublicKey) -> &[u8] {
    &public.blob()[public.blob().len() - 32..]
}

fn verifies(public: &SshPublicKey, message: &[u8], signature: &[u8]) -> bool {
    UnparsedPublicKey::new(&ED25519, raw_key(public))
        .verify(message, signature)
        .is_ok()
}

/// About the size of a real login: session id, user, key blob.
fn login_sized() -> Vec<u8> {
    (0..170u8).collect()
}

#[test]
fn reads_the_ssh_public_key_without_a_pin() {
    let key = key(true, true, false);
    let public = card_ssh_public_key(key.clone()).unwrap();

    assert_eq!(public, SshPublicKey::ed25519(&key.authentication_public_key()));
    // Every ssh-ed25519 line starts the same way: the encoded algorithm name.
    assert!(
        public
            .authorized_key()
            .starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI")
    );
    assert_eq!(key.authentication_count(), 0);
    assert_eq!(card_info(key).unwrap().pin_tries_left, 3);
}

#[test]
fn signs_a_login_once_the_key_is_touched() {
    let key = key(true, true, false);
    let public = card_ssh_public_key(key.clone()).unwrap();
    let message = login_sized();

    let asked = AtomicBool::new(false);
    let signature = card_ssh_sign(key.clone(), PIN, &message, &|| {
        asked.store(true, Ordering::SeqCst);
        key.touch();
    })
    .unwrap();

    assert!(verifies(&public, &message, &signature));
    assert!(asked.load(Ordering::SeqCst));
    assert_eq!(key.authentication_count(), 1);
}

#[test]
fn signs_data_longer_than_one_short_apdu() {
    let message: Vec<u8> = (0..700u32).map(|i| i as u8).collect();

    // Command chaining only, then extended length as a YubiKey advertises.
    for extended_length in [false, true] {
        let key = key(false, false, extended_length);
        let public = card_ssh_public_key(key.clone()).unwrap();
        let signature = card_ssh_sign(key, PIN, &message, &|| {}).unwrap();
        assert!(verifies(&public, &message, &signature), "extended: {extended_length}");
    }
}

#[test]
fn the_card_signer_signs_with_the_key_it_reports() {
    let card = key(false, true, false);
    let touch = card.clone();
    let signer = CardSigner::new(card.clone(), PIN, move || touch.touch()).unwrap();
    let message = login_sized();

    let signature = signer.sign(&message).unwrap();

    assert!(verifies(signer.public_key(), &message, &signature));
}

#[test]
fn fails_when_nobody_touches_the_key() {
    let key = key(true, true, false);

    assert!(matches!(
        card_ssh_sign(key.clone(), PIN, &login_sized(), &|| {}),
        Err(PawError::Sign(_))
    ));
    assert_eq!(key.authentication_count(), 0);
}

#[test]
fn the_authentication_slot_has_its_own_touch_policy() {
    // Decryption needs a touch, authentication does not.
    let key = key(true, false, false);

    let asked = AtomicBool::new(false);
    card_ssh_sign(key.clone(), PIN, &login_sized(), &|| asked.store(true, Ordering::SeqCst))
        .unwrap();

    assert!(!asked.load(Ordering::SeqCst));
    assert_eq!(key.authentication_count(), 1);
}

#[test]
fn wrong_pin_reports_tries_left_then_blocks() {
    let key = key(true, true, false);
    let message = login_sized();

    for expected in [2, 1] {
        match card_ssh_sign(key.clone(), "000000", &message, &|| key.touch()) {
            Err(PawError::WrongPin { tries_left }) => assert_eq!(tries_left, expected),
            other => panic!("expected WrongPin, got {other:?}"),
        }
    }
    assert!(matches!(
        card_ssh_sign(key.clone(), "000000", &message, &|| key.touch()),
        Err(PawError::PinBlocked)
    ));
    assert!(matches!(
        card_ssh_sign(key.clone(), PIN, &message, &|| key.touch()),
        Err(PawError::PinBlocked)
    ));
    assert_eq!(key.authentication_count(), 0);
}

#[test]
fn the_card_refuses_to_sign_before_the_pin_is_verified() {
    let key = key(false, false, false);

    // INTERNAL AUTHENTICATE, three bytes of data, straight after power-up.
    let response = key.exchange(vec![0x00, 0x88, 0x00, 0x00, 0x03, 1, 2, 3, 0x00]);

    assert_eq!(response, [0x69, 0x82]);
}

#[test]
fn the_card_takes_one_extended_length_command() {
    let key = key(false, false, true);
    let public = card_ssh_public_key(key.clone()).unwrap();
    let message: Vec<u8> = (0..300u32).map(|i| i as u8).collect();

    let mut verify = vec![0x00, 0x20, 0x00, 0x82, PIN.len() as u8];
    verify.extend(PIN.as_bytes());
    assert_eq!(key.exchange(verify), [0x90, 0x00]);

    // CLA INS P1 P2, 00 Lc-high Lc-low, data, Le-high Le-low.
    let mut sign = vec![0x00, 0x88, 0x00, 0x00, 0x00, 0x01, 0x2C];
    sign.extend(&message);
    sign.extend([0x00, 0x00]);
    let response = key.exchange(sign);

    assert_eq!(response.len(), 66);
    assert_eq!(response[64..], [0x90, 0x00]);
    assert!(verifies(&public, &message, &response[..64]));
}

//! SSH login signatures from a simulated OpenPGP card with an ed25519
//! authentication key, made through the app's sign-in task and checked with
//! a verifier that shares no code with it.

#![cfg(feature = "sim")]

mod key_common;

use std::sync::Arc;
use std::time::Duration;

use key_common::*;
use paw_core::ffi::sync::Setup;
use paw_core::key::KeyError;
use paw_core::task::LoginPurpose;
use paw_core::task::KeyTaskState;
use paw_core::setup::SetupState;
use paw_core::sim::{SimOptions, SimulatedKey};
use paw_core::ssh::{SshPublicKey, SshSigner};
use ring::signature::{ED25519, UnparsedPublicKey};

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
    UnparsedPublicKey::new(&ED25519, raw_key(public)).verify(message, signature).is_ok()
}

/// About the size of a real login: session id, user, key blob.
fn login_sized() -> Vec<u8> {
    (0..170u8).collect()
}

#[test]
fn reads_the_ssh_public_key_without_a_pin() {
    let key = key(true, true, false);
    let dir = tempfile::tempdir().unwrap();
    let setup = Setup::open(dir.path().to_string_lossy().into_owned()).unwrap();
    let SetupState::Address { key: summary } = setup.read_key_from_session(port_with(&key).session().unwrap()).unwrap()
    else {
        panic!("expected Address")
    };
    let line = summary.ssh_public_key;

    assert_eq!(line, SshPublicKey::ed25519(&key.authentication_public_key()).authorized_key());
    // Every ssh-ed25519 line starts the same way: the encoded algorithm name.
    assert!(line.starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI"));
    assert_eq!(key.authentication_count(), 0);
    assert_eq!(key.verify_attempts(), 0);
}

#[test]
fn signs_a_login_once_the_key_is_touched() {
    let key = key(true, true, false);
    let signer = signer(&key);
    let message = login_sized();

    let signature = signer.sign(&message).unwrap();

    assert!(verifies(&SshPublicKey::ed25519(&key.authentication_public_key()), &message, &signature));
    assert!(signer.driver.saw(is_touch));
    assert_eq!(key.authentication_count(), 1);
}

#[test]
fn signs_data_longer_than_one_short_apdu() {
    let message: Vec<u8> = (0..700u32).map(|i| i as u8).collect();

    // Command chaining only, then extended length as a YubiKey advertises.
    for extended_length in [false, true] {
        let key = key(false, false, extended_length);
        let signer = signer(&key);
        let signature = signer.sign(&message).unwrap();
        assert!(verifies(signer.public_key(), &message, &signature), "extended: {extended_length}");
    }
}

#[test]
fn the_signer_signs_with_the_key_it_reports() {
    let card = key(false, true, false);
    let signer = signer(&card);
    let message = login_sized();

    let signature = signer.sign(&message).unwrap();

    assert_eq!(*signer.public_key(), SshPublicKey::ed25519(&card.authentication_public_key()));
    assert!(verifies(signer.public_key(), &message, &signature));
}

#[test]
fn fails_when_nobody_touches_the_key() {
    // Long enough not to read as a refusal at once.
    let key = SimulatedKey::new(PIN.into(), true, 1_500);
    let driver = Driver::typist(PIN);
    let login = login(&port_with(&key), LoginPurpose::Fetch, &driver);
    let signer = login.signer().unwrap();

    assert!(matches!(signer.sign(&login_sized()), Err(KeyError::TouchTimeout)));
    assert!(matches!(finish(login.task()), KeyTaskState::Failed { error: KeyError::TouchTimeout }));
    assert_eq!(key.authentication_count(), 0);
}

#[test]
fn the_authentication_slot_has_its_own_touch_policy() {
    // Decryption needs a touch, authentication does not.
    let key = key(true, false, false);
    let signer = signer(&key);

    signer.sign(&login_sized()).unwrap();

    assert!(!signer.driver.saw(is_touch));
    assert_eq!(key.authentication_count(), 1);
}

#[test]
fn the_card_takes_one_extended_length_command() {
    let key = key(false, false, true);
    let public = SshPublicKey::ed25519(&key.authentication_public_key());
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

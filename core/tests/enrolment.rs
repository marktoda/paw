//! The enrolled-key rule (design 5.6, rule 5): once a store is set up with a
//! key, the PIN is sent to no other card. Runs on the simulated keys.

#![cfg(feature = "sim")]

mod key_common;

use std::sync::Arc;

use key_common::*;
use paw_core::key::{KeyError, KeyIdentity, KeyPort, enrolment};
use paw_core::task::LoginPurpose;
use paw_core::task::KeyTaskState;
use paw_core::sim::copies::SimClock;
use paw_core::sim::{SimOptions, SimTouch, SimulatedKey};

fn other_key() -> Arc<SimulatedKey> {
    SimulatedKey::other_with_options(SimOptions {
        pin: PIN.into(),
        decrypt_touch: false,
        auth_touch: false,
        touch_timeout: WAIT,
        extended_length: false,
    })
}

fn identity(sim: &SimulatedKey, serial: &str) -> KeyIdentity {
    KeyIdentity {
        ident: format!("0006:{serial}"),
        decryption_fingerprint: Some(sim.fingerprint_hex()),
        authentication_fingerprint: Some(sim.authentication_fingerprint_hex()),
    }
}

/// A port with the usual simulated key enrolled, and nothing plugged in.
fn enrolled_port(enrolled: &SimulatedKey) -> Arc<KeyPort> {
    let port = KeyPort::new(SimClock::new(1_790_000_000_000));
    port.set_enrolled_key(Some(identity(enrolled, "12345678")));
    port
}

#[test]
fn a_different_card_is_refused_before_any_verify() {
    let usual = sim(SimTouch::Off, SimTouch::Off, WAIT);
    let port = enrolled_port(&usual);
    let other = other_key();
    port.plugged_in(other.clone()).unwrap();
    assert!(!port.key_matches_enrolment());
    let mismatch = port.key_mismatch().unwrap();
    assert_eq!(mismatch.enrolled, identity(&usual, "12345678"));
    assert_eq!(mismatch.presented, identity(&other, "87654321"));

    // A sign-in, which needs no entry: refused at once, PIN never asked.
    let driver = Driver::helpful(PIN, &other);
    let sign_in = login(&port, LoginPurpose::Fetch, &driver);
    let task = sign_in.task();
    let state = finish(task);
    let KeyTaskState::Failed { error: ref error @ KeyError::DifferentKey { mismatch: ref refused } } = state else {
        panic!("{state:?}")
    };
    let words = error.to_string();
    assert!(words.contains("different key from the one this store was set up with"), "{words}");
    assert!(words.contains("serial 12345678") && words.contains("serial 87654321"));
    assert_eq!(**refused, mismatch);
    assert!(!driver.saw(is_pin));
    // Nothing that carries or asks about a PIN reached the card.
    assert_eq!(other.verify_attempts(), 0);
    assert_eq!(other.pin_queries(), 0);
}

#[test]
fn a_task_waiting_for_a_key_refuses_the_wrong_one_when_it_is_plugged_in() {
    let usual = sim(SimTouch::Off, SimTouch::Off, WAIT);
    let port = enrolled_port(&usual);
    let driver = Driver::helpful(PIN, &usual);
    let sign_in = paw_core::sim::tasks::SimLogin::start(&port, LoginPurpose::Clone, driver.clone());
    let task = sign_in.task();
    assert!(matches!(task.state(), KeyTaskState::PlugIn));

    let other = other_key();
    port.plugged_in(other.clone()).unwrap();
    assert!(matches!(finish(task), KeyTaskState::Failed { error: KeyError::DifferentKey { .. } }));
    assert_eq!(other.verify_attempts() + other.pin_queries(), 0);
}

#[test]
fn the_enrolled_card_proceeds() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    rig.port.set_enrolled_key(Some(identity(&rig.sim, "12345678")));
    assert!(rig.port.key_matches_enrolment());
    assert!(rig.port.key_mismatch().is_none());

    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(&rig, ENTRY, &driver);
    assert!(matches!(finish(&task), KeyTaskState::Done { .. }));
    assert_eq!(rig.sim.verify_attempts(), 1);
}

#[test]
fn a_duplicate_of_the_enrolled_key_with_another_serial_is_the_same_key() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    // Recorded from a second YubiKey loaded with the same keys.
    rig.port.set_enrolled_key(Some(identity(&rig.sim, "99999999")));
    assert!(rig.port.key_matches_enrolment());
}

#[test]
fn a_recorded_key_is_accepted_and_a_port_loading_the_record_refuses_another() {
    let usual = sim(SimTouch::Off, SimTouch::Off, WAIT);
    let other = other_key();
    let state_dir = tempfile::tempdir().unwrap();
    let dir = state_dir.path().to_string_lossy().into_owned();
    enrolment::save_enrolment(state_dir.path(), &identity(&other, "87654321")).unwrap();

    let port = KeyPort::new(SimClock::new(1_790_000_000_000));
    assert_eq!(port.load_enrolled_key(dir.clone()).unwrap(), Some(identity(&other, "87654321")));
    port.plugged_in(other.clone()).unwrap();
    assert!(port.key_matches_enrolment());

    // A sign-in on the recorded key asks for the PIN and verifies it.
    let driver = Driver::helpful(PIN, &other);
    let sign_in = login(&port, LoginPurpose::Fetch, &driver);
    sign_in.signer().unwrap();
    sign_in.finish();
    assert!(matches!(finish(sign_in.task()), KeyTaskState::Done { .. }));
    assert_eq!(other.verify_attempts(), 1);

    let fresh = KeyPort::new(SimClock::new(1_790_000_000_000));
    fresh.load_enrolled_key(dir).unwrap();
    fresh.plugged_in(usual.clone()).unwrap();
    assert!(!fresh.key_matches_enrolment());
}

#[test]
fn with_nothing_enrolled_every_card_is_accepted_and_a_damaged_record_is_an_error() {
    let port = KeyPort::new(SimClock::new(1_790_000_000_000));
    port.plugged_in(other_key()).unwrap();
    assert!(port.key_matches_enrolment());

    let state_dir = tempfile::tempdir().unwrap();
    let dir = state_dir.path().to_string_lossy().into_owned();
    assert_eq!(port.load_enrolled_key(dir.clone()).unwrap(), None);
    std::fs::write(state_dir.path().join(paw_core::files::layout::ENROLLED_KEY), "ident=0006:1\ndecryption_fingerprint=not hex\n").unwrap();
    assert!(port.load_enrolled_key(dir).is_err());
}

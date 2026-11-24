//! Key diagnostics' touch timeout check (`KeyPort::check_touch_timeout`):
//! one signature nobody touches for, through the key session, so the PIN
//! goes only where every key task sends it.

#![cfg(feature = "sim")]

mod key_common;

use std::time::Duration;

use key_common::*;
use paw_core::key::{KeyError, KeyIdentity, KeyOutcome, KeyPurpose, KeyTaskState};
use paw_core::sim::{SimTouch, SimulatedKey};

/// A plugged-in key that gives up on a touch after `timeout`. Longer than
/// a second, or the refusal is too quick to be called a timeout.
fn rig_timing_out(auth: SimTouch, timeout: Duration) -> Rig {
    let mut rig = rig_unplugged(SimTouch::EachUse, auth);
    rig.sim = sim(SimTouch::EachUse, auth, timeout);
    rig.port.attach(rig.sim.clone()).unwrap();
    rig
}

#[test]
fn an_untouched_key_fails_the_check_with_a_touch_timeout_and_the_log_has_its_answer() {
    let rig = rig_timing_out(SimTouch::EachUse, Duration::from_millis(1_200));
    let driver = Driver::typist(PIN);
    let task = rig.port.clone().check_touch_timeout(driver.clone());
    driver.drive(&task);

    assert_eq!(task.purpose(), KeyPurpose::CheckTouchTimeout);
    assert!(matches!(finish(&task), KeyTaskState::Failed { error: KeyError::TouchTimeout }));
    assert!(driver.saw(is_touch), "the sheet shows that the key is waiting");
    let session = rig.port.session().unwrap();
    assert!(session.holds_pin(), "a touch timeout keeps the PIN");
    assert_eq!(rig.sim.verify_attempts(), 1);
    assert_eq!(rig.sim.pin_tries_left(), 3);

    let log = session.diagnostics(0);
    assert!(log.contains("task touch timeout check"), "{log}");
    assert!(log.contains("refused 6982 after"), "{log}");
    assert!(log.contains("PIN check 9000: a touch timeout"), "{log}");
    assert!(!log.contains(PIN), "{log}");
}

#[test]
fn with_the_pin_held_the_check_asks_for_none() {
    let rig = rig_timing_out(SimTouch::EachUse, Duration::from_millis(1_200));
    // An open first, which takes the PIN; touched.
    let opening = Driver::helpful(PIN, &rig.sim);
    assert!(matches!(finish(&open(&rig, ENTRY, &opening)), KeyTaskState::Done { .. }));
    let driver = Driver::new();
    let task = rig.port.clone().check_touch_timeout(driver.clone());
    driver.drive(&task);

    assert!(matches!(finish(&task), KeyTaskState::Failed { error: KeyError::TouchTimeout }));
    assert!(!driver.saw(is_pin));
    assert_eq!(rig.sim.verify_attempts(), 1, "only the open's VERIFY");
}

#[test]
fn a_touched_key_or_one_that_asks_for_no_touch_signs_and_the_check_is_done() {
    let rig = rig_timing_out(SimTouch::EachUse, Duration::from_secs(5));
    let driver = Driver::helpful(PIN, &rig.sim);
    let task = rig.port.clone().check_touch_timeout(driver.clone());
    driver.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { outcome: KeyOutcome::SignedIn }));
    assert!(driver.saw(is_touch));

    let rig = rig_timing_out(SimTouch::Off, Duration::from_secs(5));
    let driver = Driver::typist(PIN);
    let task = rig.port.clone().check_touch_timeout(driver.clone());
    driver.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { outcome: KeyOutcome::SignedIn }));
    assert!(!driver.saw(is_touch), "a slot that asks for no touch never shows Touch");
}

#[test]
fn a_card_that_is_not_the_enrolled_key_is_refused_before_the_pin() {
    let rig = rig_timing_out(SimTouch::EachUse, Duration::from_millis(1_200));
    let other = SimulatedKey::new_other(PIN.into(), true, 1_000);
    let info = paw_core::card_info(other).unwrap();
    rig.port.set_enrolled_key(Some(KeyIdentity {
        ident: info.ident,
        decryption_fingerprint: info.decryption_fingerprint,
        authentication_fingerprint: info.authentication_fingerprint,
    }));

    let driver = Driver::typist(PIN);
    let task = rig.port.clone().check_touch_timeout(driver.clone());
    driver.drive(&task);

    assert!(matches!(finish(&task), KeyTaskState::Failed { error: KeyError::NotForThisKey { .. } }));
    assert!(task.key_mismatch().is_some());
    assert_eq!(rig.sim.verify_attempts(), 0);
    assert_eq!(rig.sim.pin_queries(), 0);
}

//! The key session and key tasks against a simulated key: the PIN asked
//! once per plug-in, re-verified silently, dropped when it must be; the
//! sheet's stages; cancel at every stage; one card job at a time.

#![cfg(feature = "sim")]

mod key_common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use key_common::*;
use paw_core::key::{CACHED_TOUCH_GRACE, KeyError, PIN_BACKGROUND_LIMIT};
use paw_core::task::LoginPurpose;
use paw_core::task::{KeyOutcome, KeyTaskState};
use paw_core::key::CardConnection;
use paw_core::sim::SimTouch;
use paw_core::ssh::SshSigner;
use ring::signature::{ED25519, UnparsedPublicKey};

fn opened(state: &KeyTaskState) -> (String, Option<i64>) {
    match state {
        KeyTaskState::Done { outcome: KeyOutcome::Opened { entry, face_id_until_ms, .. } } => {
            (entry.password().unwrap_or_default(), *face_id_until_ms)
        }
        other => panic!("expected an opened entry, got {other:?}"),
    }
}

fn failed(state: &KeyTaskState) -> KeyError {
    match state {
        KeyTaskState::Failed { error } => error.clone(),
        other => panic!("expected a failure, got {other:?}"),
    }
}

fn pin_stage(state: &KeyTaskState) -> (u8, bool, Option<KeyError>) {
    match state {
        KeyTaskState::Pin { tries_left, forgot_after_background, last_error } => {
            (*tries_left, *forgot_after_background, last_error.clone())
        }
        other => panic!("expected the PIN stage, got {other:?}"),
    }
}

/// Opens [`ENTRY`] with a helpful driver and returns the driver.
fn open_ok(rig: &Rig) -> Arc<Driver> {
    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(rig, ENTRY, &driver);
    assert_eq!(opened(&finish(&task)).0, SECRET);
    driver
}

// The PIN, held.

#[test]
fn pin_is_asked_once_across_two_opens() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);

    let first = open_ok(&rig);
    assert!(first.saw(is_touch));
    let second = Driver::helpful(PIN, &rig.sim);
    let task = open(&rig, OTHER_ENTRY, &second);
    assert!(!is_pin(&task.state()), "a held PIN skips the PIN stage");
    assert_eq!(opened(&finish(&task)).0, "4071");

    assert!(!second.saw(is_pin));
    assert!(second.saw(is_touch));
    assert_eq!(rig.sim.verify_attempts(), 1);
    assert_eq!(rig.sim.decipher_count(), 2);
}

#[test]
fn pin_is_held_across_an_entry_open_and_an_ssh_login() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    open_ok(&rig);

    let driver = Driver::helpful(PIN, &rig.sim);
    let login = login(&rig.port, LoginPurpose::Push { count: 2 }, &driver);
    let signer = login.signer().unwrap();
    let message: Vec<u8> = (0..146u8).collect();
    let signature = signer.sign(&message).unwrap();
    login.finish();

    let public = rig.sim.authentication_public_key();
    assert!(UnparsedPublicKey::new(&ED25519, public).verify(&message, &signature).is_ok());
    assert!(matches!(finish(login.task()), KeyTaskState::Done { outcome: KeyOutcome::SignedIn }));
    assert!(!driver.saw(is_pin));
    assert!(driver.saw(is_touch));
    assert_eq!(rig.sim.verify_attempts(), 1);
    assert_eq!(rig.sim.authentication_count(), 1);
}

#[test]
fn silent_reverify_after_the_card_drops_the_pin_costs_no_prompt_and_one_verify() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    open_ok(&rig);
    // Every card job starts with a SELECT; from now on that drops the PIN.
    rig.sim.set_reselect_drops_pin(true);
    let queries = rig.sim.pin_queries();

    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(&rig, OTHER_ENTRY, &driver);
    assert_eq!(opened(&finish(&task)).0, "4071");

    assert!(!driver.saw(is_pin));
    assert_eq!(rig.sim.verify_attempts(), 2, "exactly one VERIFY for the second open");
    assert_eq!(rig.sim.pin_queries(), queries + 1, "it asked the card first");
}

#[test]
fn a_card_still_verified_is_not_verified_again() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    open_ok(&rig);
    let task = open(&rig, OTHER_ENTRY, &Driver::new());
    finish(&task);
    assert_eq!(rig.sim.verify_attempts(), 1);
}

#[test]
fn pin_changed_elsewhere_drops_the_held_pin_after_one_try() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    open_ok(&rig);
    rig.sim.change_pin_elsewhere("246810".into());
    let session = rig.port.session().unwrap();

    let driver = Driver::new();
    let task = open(&rig, OTHER_ENTRY, &driver);
    driver.wait_for(is_pin);

    let (tries_left, forgot, last_error) = pin_stage(&task.state());
    assert_eq!(last_error, Some(KeyError::HeldPinRejected { tries_left: 2 }));
    assert_eq!(tries_left, 2);
    assert!(!forgot);
    assert!(!session.holds_pin());
    assert_eq!(rig.sim.verify_attempts(), 2, "one try spent on the old PIN, no more");

    task.supply_pin(b"246810".to_vec());
    assert_eq!(opened(&finish(&task)).0, "4071");
    assert_eq!(rig.sim.verify_attempts(), 3);
    assert!(session.holds_pin());
}

#[test]
fn fewer_than_three_tries_are_shown_before_input() {
    let rig = rig_unplugged(SimTouch::Off, SimTouch::Off);
    rig.sim.set_pin_tries_left(2);
    rig.port.plugged_in(rig.sim.clone()).unwrap();

    let task = open(&rig, ENTRY, &Driver::new());
    assert_eq!(pin_stage(&task.state()), (2, false, None));
    assert_eq!(rig.sim.verify_attempts(), 0);
    task.cancel();

    let rig = rig_unplugged(SimTouch::Off, SimTouch::Off);
    rig.port.plugged_in(rig.sim.clone()).unwrap();
    let task = open(&rig, ENTRY, &Driver::new());
    assert_eq!(pin_stage(&task.state()), (3, false, None), "three tries: nothing to warn about");
    task.cancel();
}

#[test]
fn wrong_pin_counts_down_then_blocks_and_blocked_is_reported_without_a_try() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    let driver = Driver::new();
    let task = open(&rig, ENTRY, &driver);

    task.supply_pin(b"000000".to_vec());
    driver.wait_for(|s| matches!(s, KeyTaskState::Pin { last_error: Some(_), .. }));
    assert_eq!(pin_stage(&task.state()), (2, false, Some(KeyError::WrongPin { tries_left: 2 })));

    task.supply_pin(b"000000".to_vec());
    driver.wait_for(|s| matches!(s, KeyTaskState::Pin { last_error: Some(KeyError::WrongPin { tries_left: 1 }), .. }));
    assert_eq!(pin_stage(&task.state()).0, 1);

    task.supply_pin(b"000000".to_vec());
    assert_eq!(failed(&finish(&task)), KeyError::PinBlocked);
    assert_eq!(rig.sim.verify_attempts(), 3);
    assert!(!driver.saw(is_touch), "a wrong PIN never leads to a touch");

    let queries = rig.sim.pin_queries();
    let again = open(&rig, ENTRY, &Driver::new());
    assert_eq!(failed(&again.state()), KeyError::PinBlocked);
    assert_eq!(rig.sim.verify_attempts(), 3);
    assert_eq!(rig.sim.pin_queries(), queries);
}

#[test]
fn a_pin_of_the_wrong_length_is_refused_without_a_try() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    let driver = Driver::new();
    let task = open(&rig, ENTRY, &driver);

    for pin in [Vec::new(), b"12345".to_vec(), vec![b'1'; 128]] {
        task.supply_pin(pin);
        driver.wait_for(|s| matches!(s, KeyTaskState::Pin { last_error: Some(KeyError::PinFormat), .. }));
        assert_eq!(rig.sim.verify_attempts(), 0);
    }
    task.supply_pin(PIN.as_bytes().to_vec());
    assert_eq!(opened(&finish(&task)).0, SECRET);
    assert_eq!(rig.sim.verify_attempts(), 1);
}

// The background limit.

#[test]
fn fourteen_minutes_in_the_background_keeps_the_pin_fifteen_forget_it() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    open_ok(&rig);
    let session = rig.port.session().unwrap();
    let minute = 60_000;

    session.note_backgrounded();
    rig.clock.advance_ms(14 * minute);
    session.note_foregrounded();
    assert!(session.holds_pin());
    let driver = Driver::new();
    finish(&open(&rig, OTHER_ENTRY, &driver));
    assert!(!driver.saw(is_pin));

    session.note_backgrounded();
    rig.clock.advance_ms(PIN_BACKGROUND_LIMIT.as_millis() as u64);
    session.note_foregrounded();
    assert!(!session.holds_pin());
    assert!(eventually(|| !rig.sim.pin_verified()), "the card was de-verified too");

    let task = open(&rig, OTHER_ENTRY, &Driver::new());
    assert_eq!(pin_stage(&task.state()), (3, true, None));
    task.supply_pin(PIN.as_bytes().to_vec());
    finish(&task);
    assert_eq!(rig.sim.verify_attempts(), 2);

    // Once typed again, the note goes away.
    let task = open(&rig, ENTRY, &Driver::new());
    finish(&task);
    session.forget_pin();
    let task = open(&rig, ENTRY, &Driver::new());
    assert_eq!(pin_stage(&task.state()), (3, false, None));
    task.cancel();
}

#[test]
fn the_background_limit_holds_even_if_coming_back_was_not_noted() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    open_ok(&rig);
    let session = rig.port.session().unwrap();
    session.note_backgrounded();
    rig.clock.advance_ms(PIN_BACKGROUND_LIMIT.as_millis() as u64 + 1);

    let task = open(&rig, ENTRY, &Driver::new());
    assert_eq!(pin_stage(&task.state()), (3, true, None));
    task.cancel();
}

#[test]
fn forget_pin_asks_again_and_deverifies_the_card() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    open_ok(&rig);
    assert!(rig.sim.pin_verified());
    rig.port.session().unwrap().forget_pin();
    assert!(eventually(|| !rig.sim.pin_verified()));

    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(&rig, ENTRY, &driver);
    finish(&task);
    assert_eq!(rig.sim.verify_attempts(), 2, "the PIN was asked and verified again");
}

/// A transport that holds the de-verify (`00 20 FF 82`) until released,
/// as a slow or stuck reader would.
struct HeldDeverify {
    inner: Arc<paw_core::sim::SimulatedKey>,
    gate: std::sync::Mutex<bool>,
    opened: std::sync::Condvar,
    arrived: std::sync::atomic::AtomicBool,
}

impl HeldDeverify {
    fn release(&self) {
        *self.gate.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

impl CardConnection for HeldDeverify {
    fn transmit(&self, apdu: Vec<u8>) -> Result<Vec<u8>, paw_core::key::ConnectionError> {
        if apdu.starts_with(&[0x00, 0x20, 0xFF, 0x82]) {
            self.arrived.store(true, std::sync::atomic::Ordering::SeqCst);
            let mut open = self.gate.lock().unwrap();
            while !*open {
                open = self.opened.wait(open).unwrap();
            }
        }
        self.inner.transmit(apdu)
    }
}

#[test]
fn forget_pin_returns_at_once_while_the_reader_is_slow_and_deverifies_after() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    let held = Arc::new(HeldDeverify {
        inner: rig.sim.clone(),
        gate: std::sync::Mutex::new(false),
        opened: std::sync::Condvar::new(),
        arrived: std::sync::atomic::AtomicBool::new(false),
    });
    let session = rig.port.plugged_in(held.clone()).unwrap();
    open_ok(&rig);
    assert!(rig.sim.pin_verified());

    let started = Instant::now();
    session.forget_pin();
    assert!(started.elapsed() < Duration::from_secs(2), "forget_pin waited for the card");
    assert!(!session.holds_pin(), "the PIN is gone from memory before forget_pin returns");
    assert!(eventually(|| held.arrived.load(std::sync::atomic::Ordering::SeqCst)), "the de-verify was sent");
    assert!(rig.sim.pin_verified(), "the reader has not answered yet");

    // A task started meanwhile waits for the card, then asks for the PIN.
    let driver = Driver::new();
    let task = open(&rig, ENTRY, &driver);
    held.release();
    assert!(eventually(|| !rig.sim.pin_verified()), "the de-verify went through afterwards");
    assert!(eventually(|| is_pin(&task.state())));
    task.cancel();
}

#[test]
fn holds_pin_after_the_background_limit_answers_without_the_card() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    let held = Arc::new(HeldDeverify {
        inner: rig.sim.clone(),
        gate: std::sync::Mutex::new(false),
        opened: std::sync::Condvar::new(),
        arrived: std::sync::atomic::AtomicBool::new(false),
    });
    let session = rig.port.plugged_in(held.clone()).unwrap();
    open_ok(&rig);
    session.note_backgrounded();
    rig.clock.advance_ms(PIN_BACKGROUND_LIMIT.as_millis() as u64);

    let started = Instant::now();
    assert!(!session.holds_pin());
    session.note_foregrounded();
    assert!(started.elapsed() < Duration::from_secs(2), "holds_pin waited for the card");
    held.release();
    assert!(eventually(|| !rig.sim.pin_verified()));
}

#[test]
fn forget_pin_during_a_touch_wait_returns_at_once_and_deverifies_after_the_command() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    open_ok(&rig);
    let session = rig.port.session().unwrap();

    let driver = Driver::new();
    let task = open(&rig, OTHER_ENTRY, &driver);
    driver.wait_for(is_touch);
    assert!(eventually(|| rig.sim.waiting_for_touch()));

    let started = Instant::now();
    session.forget_pin();
    assert!(started.elapsed() < Duration::from_secs(2), "forget_pin waited for the touch");
    assert!(!session.holds_pin());
    assert!(rig.sim.pin_verified(), "the card is still busy with the decrypt");

    rig.sim.touch();
    finish(&task);
    assert!(eventually(|| !rig.sim.pin_verified()), "de-verified once the command ended");
}

#[test]
fn time_out_touch_ends_the_wait_as_the_keys_timeout_would() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    open_ok(&rig);
    let driver = Driver::new();
    let task = open(&rig, OTHER_ENTRY, &driver);
    driver.wait_for(is_touch);
    assert!(eventually(|| rig.sim.waiting_for_touch()));
    let started = Instant::now();
    rig.sim.time_out_touch();
    assert_eq!(failed(&finish(&task)), KeyError::TouchTimeout);
    assert!(started.elapsed() < Duration::from_secs(2));
    // The PIN is still held, and the next touch works.
    let task = open(&rig, OTHER_ENTRY, &Driver::helpful(PIN, &rig.sim));
    assert_eq!(opened(&finish(&task)).0, "4071");
}

// Removal.

#[test]
fn removal_mid_touch_fails_as_key_removed_and_drops_the_pin() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    open_ok(&rig);
    let session = rig.port.session().unwrap();

    let driver = Driver::new();
    let task = open(&rig, OTHER_ENTRY, &driver);
    driver.wait_for(is_touch);
    assert!(eventually(|| rig.sim.waiting_for_touch()));
    rig.sim.unplug();

    assert_eq!(failed(&finish(&task)), KeyError::KeyRemoved);
    assert!(!session.holds_pin());
    assert!(rig.port.session().is_none());

    rig.sim.plug_in();
    let session = rig.port.plugged_in(rig.sim.clone()).unwrap();
    assert!(!session.holds_pin());
    let task = open(&rig, OTHER_ENTRY, &Driver::new());
    assert!(is_pin(&task.state()), "the PIN is asked again after a re-plug");
    task.cancel();
}

#[test]
fn unplugging_at_the_pin_stage_fails_as_key_removed() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    let task = open(&rig, ENTRY, &Driver::new());
    assert!(is_pin(&task.state()));
    rig.port.unplugged();
    assert_eq!(failed(&finish(&task)), KeyError::KeyRemoved);
}

#[test]
fn a_task_started_with_no_key_waits_at_plug_in_and_goes_on() {
    let rig = rig_unplugged(SimTouch::EachUse, SimTouch::EachUse);
    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(&rig, ENTRY, &driver);
    assert!(matches!(task.state(), KeyTaskState::PlugIn));

    rig.port.plugged_in(rig.sim.clone()).unwrap();
    assert_eq!(opened(&finish(&task)).0, SECRET);
    assert!(driver.saw(is_pin));
}

// Cancel, at every stage.

/// Cancels and checks the task says so at once and for good.
fn cancel_now(task: &Arc<paw_core::task::KeyTask>, driver: &Driver) {
    task.cancel();
    assert!(matches!(task.state(), KeyTaskState::Cancelled));
    assert!(!task.join(), "the task's thread ended without a panic");
    assert!(matches!(task.state(), KeyTaskState::Cancelled));
    assert!(matches!(driver.states().last(), Some(KeyTaskState::Cancelled)));
}

#[test]
fn cancel_at_plug_in() {
    let rig = rig_unplugged(SimTouch::Off, SimTouch::Off);
    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(&rig, ENTRY, &driver);
    cancel_now(&task, &driver);
    rig.port.plugged_in(rig.sim.clone()).unwrap();
    assert!(matches!(task.state(), KeyTaskState::Cancelled));
    assert_eq!(rig.sim.verify_attempts(), 0);
}

#[test]
fn cancel_at_the_pin_stage() {
    let rig = rig(SimTouch::Off, SimTouch::Off);
    let driver = Driver::new();
    let task = open(&rig, ENTRY, &driver);
    cancel_now(&task, &driver);
    task.supply_pin(PIN.as_bytes().to_vec());
    assert_eq!(rig.sim.verify_attempts(), 0);
}

#[test]
fn cancel_while_busy_leaves_the_running_task_alone() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    open_ok(&rig);
    let first_driver = Driver::new();
    let first = open(&rig, ENTRY, &first_driver);
    first_driver.wait_for(is_touch);

    let second_driver = Driver::new();
    let second = open(&rig, OTHER_ENTRY, &second_driver);
    assert!(is_busy(&second.state()));
    cancel_now(&second, &second_driver);

    rig.sim.touch();
    assert_eq!(opened(&finish(&first)).0, SECRET);
    assert!(matches!(second.state(), KeyTaskState::Cancelled));
    assert_eq!(rig.sim.decipher_count(), 2);
}

#[test]
fn cancel_while_working_stops_a_sign_in_before_it_signs() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    let driver = Driver::helpful(PIN, &rig.sim);
    let login = login(&rig.port, LoginPurpose::Fetch, &driver);
    let signer = login.signer().unwrap();
    assert!(matches!(login.task().state(), KeyTaskState::Working { .. }));

    cancel_now(login.task(), &driver);
    assert!(signer.sign(b"login").is_err());
    assert_eq!(rig.sim.authentication_count(), 0);
    assert!(matches!(finish(login.task()), KeyTaskState::Cancelled));
}

#[test]
fn a_decrypt_finishing_after_cancel_leaks_nothing_and_keeps_no_copy() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    let first_driver = Driver::helpful(PIN, &rig.sim);
    finish(&open(&rig, OTHER_ENTRY, &first_driver));

    let driver = Driver::new();
    let task = rig.port.clone().open_entry(rig.store.clone(), ENTRY.into(), Some(rig.copies.clone()), 3600, driver.clone());
    driver.drive(&task);
    driver.wait_for(is_touch);
    assert!(eventually(|| rig.sim.waiting_for_touch()));
    task.cancel();
    assert!(matches!(task.state(), KeyTaskState::Cancelled), "cancelled at once, mid-touch");

    // The card is still holding the cancelled command: the next task waits.
    let next_driver = Driver::toucher(&rig.sim);
    let next = rig.port.clone().open_entry(rig.store.clone(), OTHER_ENTRY.into(), Some(rig.copies.clone()), 3600, next_driver.clone());
    next_driver.drive(&next);
    assert!(is_busy(&next.state()));
    assert!(!next_driver.saw(is_touch));

    // The late touch releases the cancelled entry, which is thrown away.
    rig.sim.touch();
    assert!(!task.join());
    assert!(matches!(task.state(), KeyTaskState::Cancelled));
    assert!(!driver.saw(is_done));
    assert_eq!(opened(&finish(&next)).0, "4071");
    assert!(next_driver.saw(is_touch));
    assert_eq!(rig.sim.decipher_count(), 3);

    let copies: Vec<String> = rig.copies.list().unwrap().into_iter().map(|c| c.entry).collect();
    assert_eq!(copies, vec![OTHER_ENTRY.to_string()], "no copy of the cancelled entry");
    for state in driver.states() {
        assert!(!format!("{state:?}").contains(SECRET));
    }
}

// Refused before anything.

#[test]
fn an_entry_for_another_key_is_refused_with_no_verify_and_no_decipher() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    let queries = rig.sim.pin_queries();
    let task = open(&rig, FOREIGN_ENTRY, &Driver::new());

    match failed(&task.state()) {
        KeyError::NotForThisKey { recipients } => {
            assert_eq!(recipients.len(), 1);
            assert!(matches!(recipients[0], paw_core::recipients::Recipient::KeyId { .. }), "{recipients:?}");
        }
        other => panic!("expected NotForThisKey, got {other:?}"),
    }
    assert_eq!(rig.sim.verify_attempts(), 0);
    assert_eq!(rig.sim.pin_queries(), queries);
    assert_eq!(rig.sim.decipher_count(), 0);

    // Also when the key is plugged in only later.
    let rig = rig_unplugged(SimTouch::EachUse, SimTouch::EachUse);
    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(&rig, FOREIGN_ENTRY, &driver);
    rig.port.plugged_in(rig.sim.clone()).unwrap();
    assert!(matches!(failed(&finish(&task)), KeyError::NotForThisKey { .. }));
    assert!(!driver.saw(is_pin));
    assert_eq!(rig.sim.verify_attempts(), 0);
}

// Touch.

#[test]
fn touch_is_shown_only_once_the_command_goes_out() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    let driver = Driver::new();
    let task = open(&rig, ENTRY, &driver);
    task.supply_pin(PIN.as_bytes().to_vec());
    driver.wait_for(is_touch);

    // By then the PIN was verified, nothing was decrypted, and the card is
    // (or is about to be) holding this task's command.
    assert_eq!(rig.sim.verify_attempts(), 1);
    assert_eq!(rig.sim.decipher_count(), 0);
    assert!(eventually(|| rig.sim.waiting_for_touch()));
    rig.sim.touch();
    finish(&task);

    let rig = rig_unplugged(SimTouch::Off, SimTouch::Off);
    rig.port.plugged_in(rig.sim.clone()).unwrap();
    let driver = Driver::helpful(PIN, &rig.sim);
    finish(&open(&rig, ENTRY, &driver));
    assert!(!driver.saw(is_touch), "a slot without a touch policy never shows Touch");
}

#[test]
fn a_touch_timeout_fails_the_task_and_keeps_the_pin() {
    let mut rig = rig_unplugged(SimTouch::EachUse, SimTouch::EachUse);
    // Past `FAST_REFUSAL`: a quicker refusal is not taken for a timeout.
    rig.sim = sim(SimTouch::EachUse, SimTouch::EachUse, Duration::from_millis(1_100));
    rig.port.plugged_in(rig.sim.clone()).unwrap();
    let driver = Driver::new();
    let task = open(&rig, ENTRY, &driver);
    task.supply_pin(PIN.as_bytes().to_vec());
    assert_eq!(failed(&finish(&task)), KeyError::TouchTimeout);
    assert!(driver.saw(is_touch));

    // Try again: straight to Touch, no PIN, no try used.
    let driver = Driver::toucher(&rig.sim);
    let task = open(&rig, ENTRY, &driver);
    assert!(!is_pin(&task.state()));
    assert_eq!(opened(&finish(&task)).0, SECRET);
    assert_eq!(rig.sim.verify_attempts(), 1);
    assert_eq!(rig.sim.pin_tries_left(), 3);
}

#[test]
fn a_cached_touch_does_not_flash_a_prompt() {
    let rig = rig(SimTouch::Off, SimTouch::Cached);
    let driver = Driver::helpful(PIN, &rig.sim);
    let login = login(&rig.port, LoginPurpose::Clone, &driver);
    let signer = login.signer().unwrap();

    // The first login waits for a touch: Touch comes, after the grace.
    let started = Instant::now();
    signer.sign(b"first login").unwrap();
    assert!(started.elapsed() >= CACHED_TOUCH_GRACE);
    assert_eq!(driver.count(is_touch), 1);

    // The second is covered by the first touch: no prompt at all.
    signer.sign(b"second login").unwrap();
    assert_eq!(driver.count(is_touch), 1);
    assert_eq!(rig.sim.authentication_count(), 2);
    login.finish();
    assert!(is_done(&finish(login.task())));
}

#[test]
fn a_cached_touch_that_has_run_out_shows_the_prompt_while_still_waiting() {
    let rig = rig(SimTouch::Off, SimTouch::Cached);
    rig.sim.set_touch_cache_ms(0);
    let driver = Driver::helpful(PIN, &rig.sim);
    let login = login(&rig.port, LoginPurpose::Clone, &driver);
    let signer = login.signer().unwrap();
    signer.sign(b"first login").unwrap();
    signer.sign(b"second login").unwrap();
    assert_eq!(driver.count(is_touch), 2);
}

// One card job at a time.

#[test]
fn a_second_task_queues_behind_the_first_and_shows_busy() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    open_ok(&rig);
    let first_driver = Driver::new();
    let first = open(&rig, ENTRY, &first_driver);
    first_driver.wait_for(is_touch);

    let second_driver = Driver::toucher(&rig.sim);
    let second = open(&rig, OTHER_ENTRY, &second_driver);
    assert!(is_busy(&second.state()));
    assert!(!second_driver.saw(is_touch));

    rig.sim.touch();
    assert_eq!(opened(&finish(&first)).0, SECRET);
    assert_eq!(opened(&finish(&second)).0, "4071");
    let states = second_driver.states();
    assert!(states.iter().position(is_touch).is_some());
}

// Other purposes.

#[test]
fn an_open_keeps_a_face_id_copy_when_asked() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    let driver = Driver::helpful(PIN, &rig.sim);
    let task = rig.port.clone().open_entry(rig.store.clone(), ENTRY.into(), Some(rig.copies.clone()), 3600, driver.clone());
    driver.drive(&task);
    let (password, until) = opened(&finish(&task));
    assert_eq!(password, SECRET);
    assert_eq!(until, Some(rig.clock.wall_ms() + 3_600_000));
    assert_eq!(rig.copies.list().unwrap().len(), 1);
}

// Secrets in formatted output.

#[test]
fn no_state_or_error_shows_the_pin_or_the_plaintext() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    let wrong = "918273";
    let driver = Driver::toucher(&rig.sim);
    let task = open(&rig, ENTRY, &driver);
    task.supply_pin(wrong.as_bytes().to_vec());
    driver.wait_for(|s| matches!(s, KeyTaskState::Pin { last_error: Some(_), .. }));
    task.supply_pin(PIN.as_bytes().to_vec());
    let done = finish(&task);
    opened(&done);

    let mut shown: Vec<String> = driver.states().iter().map(|s| format!("{s:?} {s:#?}")).collect();
    shown.push(format!("{done:?}"));
    for error in [
        KeyError::WrongPin { tries_left: 2 },
        KeyError::HeldPinRejected { tries_left: 1 },
        KeyError::PinFormat,
        KeyError::TouchTimeout,
        KeyError::KeyRemoved,
    ] {
        shown.push(format!("{error} {error:?}"));
    }
    let as_bytes = |s: &str| format!("{:?}", s.as_bytes()).trim_matches(['[', ']']).to_owned();
    for text in &shown {
        for secret in [PIN, wrong, SECRET, "you@example.com"] {
            assert!(!text.contains(secret), "{text} shows a secret");
            assert!(!text.contains(&as_bytes(secret)), "{text} shows a secret's bytes");
        }
    }
}

// A crafted entry.

#[test]
fn an_entry_with_many_packets_for_this_key_costs_one_decipher() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    let genuine = std::fs::read(rig.store_dir().join(format!("{ENTRY}.gpg"))).unwrap();
    std::fs::write(rig.store_dir().join("web/crafted.gpg"), with_packet_repeated(&genuine, 5)).unwrap();

    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(&rig, "web/crafted", &driver);
    let error = failed(&finish(&task));

    assert!(matches!(error, KeyError::DecryptFailed { .. }), "{error:?}");
    assert_eq!(rig.sim.decipher_count(), 1);
    assert_eq!(driver.count(is_touch), 1);
    // The genuine entry still opens with one.
    open_ok(&rig);
    assert_eq!(rig.sim.decipher_count(), 2);
}

// A key with the PIN KDF turned on.

#[test]
fn a_key_with_a_pin_kdf_is_sent_the_derived_pin_and_wastes_no_try() {
    for kdf in [false, true] {
        let rig = rig_unplugged(SimTouch::Off, SimTouch::Off);
        if kdf {
            rig.sim.set_pin_kdf(3);
        }
        rig.port.plugged_in(rig.sim.clone()).unwrap();

        open_ok(&rig);

        assert_eq!(rig.sim.verify_attempts(), 1, "kdf: {kdf}");
        assert_eq!(rig.sim.pin_tries_left(), 3, "kdf: {kdf}");
    }
}

#[test]
fn a_key_with_a_pin_kdf_paw_does_not_know_is_refused_before_the_pin() {
    let rig = rig_unplugged(SimTouch::Off, SimTouch::Off);
    rig.sim.set_pin_kdf(7);
    rig.port.plugged_in(rig.sim.clone()).unwrap();
    let driver = Driver::helpful(PIN, &rig.sim);

    let task = open(&rig, ENTRY, &driver);
    let error = failed(&finish(&task));

    assert!(matches!(&error, KeyError::Reader { detail } if detail.contains("KDF")), "{error:?}");
    assert!(!driver.saw(is_pin));
    assert_eq!(rig.sim.verify_attempts(), 0);
    assert_eq!(rig.sim.pin_tries_left(), 3);
}

// A refusal that may be a touch timeout, or a PIN the card lost.

/// The simulated key behind a transport that, a set number of times, makes
/// the card forget the PIN's verification just before PSO:DECIPHER or
/// INTERNAL AUTHENTICATE goes out (as a reset of the card mid-job would),
/// or answers that command with a refusal at once.
struct LosesPin {
    sim: Arc<paw_core::sim::SimulatedKey>,
    losses: AtomicU32,
    refuse_at_once: AtomicU32,
}

impl LosesPin {
    fn new(sim: &Arc<paw_core::sim::SimulatedKey>) -> Arc<Self> {
        Arc::new(Self { sim: sim.clone(), losses: AtomicU32::new(0), refuse_at_once: AtomicU32::new(0) })
    }
}

fn take(counter: &AtomicU32) -> bool {
    counter.load(Ordering::SeqCst) > 0 && counter.fetch_sub(1, Ordering::SeqCst) > 0
}

impl CardConnection for LosesPin {
    fn transmit(&self, apdu: Vec<u8>) -> Result<Vec<u8>, paw_core::key::ConnectionError> {
        let key_operation = matches!(apdu.as_slice(), [cla, 0x2A, 0x80, 0x86, ..] | [cla, 0x88, ..] if cla & 0x10 == 0);
        if key_operation && take(&self.refuse_at_once) {
            return Ok(vec![0x69, 0x82]);
        }
        if key_operation && take(&self.losses) {
            self.sim.transmit(vec![0x00, 0x20, 0xFF, 0x82])?;
        }
        self.sim.transmit(apdu)
    }
}

fn rig_losing_pin(decrypt: SimTouch, auth: SimTouch) -> (Rig, Arc<LosesPin>) {
    let rig = rig_unplugged(decrypt, auth);
    let wrapped = LosesPin::new(&rig.sim);
    rig.port.plugged_in(wrapped.clone()).unwrap();
    (rig, wrapped)
}

fn pin_lost(state: &KeyTaskState) -> bool {
    matches!(state, KeyTaskState::Pin { last_error: Some(KeyError::PinForgotten), .. })
}

#[test]
fn a_pin_the_key_lost_before_decipher_is_asked_again_after_touch_with_no_try_spent() {
    let (rig, wrapped) = rig_losing_pin(SimTouch::EachUse, SimTouch::EachUse);
    open_ok(&rig);
    let session = rig.port.session().unwrap();
    assert!(session.holds_pin());
    wrapped.losses.store(1, Ordering::SeqCst);

    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(&rig, OTHER_ENTRY, &driver);
    assert!(!is_pin(&task.state()), "the PIN was held when the task started");
    assert_eq!(opened(&finish(&task)).0, "4071");

    let states = driver.states();
    let touch = states.iter().position(is_touch).expect("Touch was shown");
    let asked = states.iter().position(pin_lost).expect("the PIN was asked again with a reason");
    assert!(touch < asked, "Touch, then the PIN: {states:?}");
    assert_eq!(states.iter().filter(|s| is_pin(s)).count(), 1);
    assert!(states[asked..].iter().any(is_touch), "a touch after the PIN");
    assert!(!states.iter().any(|s| matches!(s, KeyTaskState::Failed { .. })));
    // One VERIFY for the first open and one for the PIN typed again; the
    // held PIN was not re-sent, and the counter never moved.
    assert_eq!(rig.sim.verify_attempts(), 2);
    assert_eq!(rig.sim.pin_tries_left(), 3);
    assert!(session.holds_pin());
    let log = session.diagnostics(0);
    assert!(log.contains("refused 6982 after"), "{log}");
    assert!(log.contains("PIN check 63C3: the PIN lost, ask again"), "{log}");
    assert!(log.contains("PIN dropped (the key lost its verification)"), "{log}");
    assert!(log.contains("stage Pin (again: PinForgotten)"), "{log}");
}

#[test]
fn a_pin_lost_on_the_first_open_is_asked_again_too() {
    let (rig, wrapped) = rig_losing_pin(SimTouch::Off, SimTouch::Off);
    wrapped.losses.store(1, Ordering::SeqCst);
    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(&rig, ENTRY, &driver);
    assert_eq!(opened(&finish(&task)).0, SECRET);
    assert_eq!(driver.count(pin_lost), 1);
    assert_eq!(rig.sim.verify_attempts(), 2);
    assert_eq!(rig.sim.pin_tries_left(), 3);
}

#[test]
fn a_second_pin_loss_in_one_task_fails_instead_of_asking_again() {
    let (rig, wrapped) = rig_losing_pin(SimTouch::EachUse, SimTouch::EachUse);
    open_ok(&rig);
    wrapped.losses.store(5, Ordering::SeqCst);

    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(&rig, OTHER_ENTRY, &driver);
    let error = failed(&finish(&task));

    assert!(matches!(&error, KeyError::Reader { detail } if detail.contains("lost the PIN twice")), "{error:?}");
    assert_eq!(driver.count(pin_lost), 1, "asked again once, not in a loop");
    assert_eq!(rig.sim.verify_attempts(), 2);
    assert_eq!(rig.sim.pin_tries_left(), 3);
    assert!(!rig.port.session().unwrap().holds_pin());
    assert_eq!(wrapped.losses.load(Ordering::SeqCst), 3, "two key operations went out, no more");
}

#[test]
fn a_quick_refusal_with_the_pin_still_verified_is_not_called_a_touch_timeout() {
    let (rig, wrapped) = rig_losing_pin(SimTouch::EachUse, SimTouch::EachUse);
    open_ok(&rig);
    wrapped.refuse_at_once.store(1, Ordering::SeqCst);

    let driver = Driver::helpful(PIN, &rig.sim);
    let error = failed(&finish(&open(&rig, OTHER_ENTRY, &driver)));

    assert!(matches!(&error, KeyError::Reader { detail } if detail.contains("refused at once (6982)")), "{error:?}");
    assert!(!driver.saw(is_pin));
    assert!(rig.port.session().unwrap().holds_pin(), "the PIN is still verified, so it is kept");
    assert_eq!(rig.sim.verify_attempts(), 1);
}

#[test]
fn a_slow_refusal_with_the_pin_still_verified_is_a_touch_timeout() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    open_ok(&rig);
    let queries = rig.sim.pin_queries();
    let driver = Driver::new();
    let task = open(&rig, OTHER_ENTRY, &driver);
    driver.wait_for(is_touch);
    assert!(eventually(|| rig.sim.waiting_for_touch()));
    rig.sim.time_out_touch();
    assert_eq!(failed(&finish(&task)), KeyError::TouchTimeout);
    assert!(rig.port.session().unwrap().holds_pin());
    // The job's own check, then the one after the refusal: no try.
    assert_eq!(rig.sim.pin_queries(), queries + 2);
    assert_eq!(rig.sim.verify_attempts(), 1);
}

#[test]
fn a_pin_lost_before_a_signature_fails_it_with_pin_forgotten() {
    let (rig, wrapped) = rig_losing_pin(SimTouch::EachUse, SimTouch::EachUse);
    let driver = Driver::helpful(PIN, &rig.sim);
    let login = login(&rig.port, LoginPurpose::Push { count: 1 }, &driver);
    let signer = login.signer().unwrap();
    wrapped.losses.store(1, Ordering::SeqCst);

    let error = signer.sign(b"a login").unwrap_err();

    assert_eq!(error, KeyError::PinForgotten);
    assert_eq!(failed(&finish(login.task())), KeyError::PinForgotten);
    assert!(!rig.port.session().unwrap().holds_pin());
    assert_eq!(rig.sim.verify_attempts(), 1);
    assert_eq!(rig.sim.pin_tries_left(), 3);
    assert_eq!(rig.sim.authentication_count(), 0);
}

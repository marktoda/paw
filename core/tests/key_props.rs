//! Whatever a card (or something posing as one) answers, the key session
//! does not panic: it fails the task, or carries on.

#![cfg(feature = "sim")]

mod key_common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use key_common::*;
use paw_core::key::{CardConnection, KeyPort};
use paw_core::task::LoginPurpose;
use paw_core::sim::copies::SimClock;
use paw_core::sim::{SimTouch, SimulatedKey};
use proptest::prelude::*;

/// A simulated key whose answers are replaced, exchange by exchange, by
/// the bytes in `script` (`None` lets the real answer through; running
/// past the end does too).
struct Tampered {
    sim: Arc<SimulatedKey>,
    script: Mutex<std::vec::IntoIter<Option<Vec<u8>>>>,
}

impl CardConnection for Tampered {
    fn transmit(&self, apdu: Vec<u8>) -> Result<Vec<u8>, paw_core::key::ConnectionError> {
        let real = self.sim.transmit(apdu);
        match self.script.lock().unwrap().next() {
            Some(Some(answer)) if answer == b"fail" => Err(paw_core::key::ConnectionError::Failed { message: "reader failed".into() }),
            Some(Some(answer)) => Ok(answer),
            _ => real,
        }
    }
}

/// Whether `task` ends within `timeout`.
fn ends_within(task: &Arc<paw_core::task::KeyTask>, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if task.state().is_final() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

fn answer() -> impl Strategy<Value = Option<Vec<u8>>> {
    prop_oneof![
        12 => Just(None),
        1 => Just(Some(b"fail".to_vec())),
        1 => prop::collection::vec(any::<u8>(), 0..4).prop_map(Some),
        1 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| Some(vec![a, b])),
        1 => prop::sample::select(vec![0x6982u16, 0x6983, 0x63C0, 0x63C1, 0x6A80, 0x6D00, 0x6100, 0x9000])
            .prop_map(|sw| Some(sw.to_be_bytes().to_vec())),
        1 => prop::collection::vec(any::<u8>(), 0..300).prop_map(Some),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    #[test]
    fn arbitrary_card_answers_never_panic_the_session(script in prop::collection::vec(answer(), 0..80)) {
        let rig = rig_unplugged(SimTouch::EachUse, SimTouch::Cached);
        let sim = sim(SimTouch::EachUse, SimTouch::Cached, Duration::from_millis(30));
        let card = Arc::new(Tampered { sim: sim.clone(), script: Mutex::new(script.into_iter()) });
        let port = KeyPort::new(SimClock::new(0));
        if port.plugged_in(card).is_err() {
            return Ok(());
        }
        for entry in [ENTRY, FOREIGN_ENTRY] {
            let driver = Driver::helpful(PIN, &sim);
            let task = port.clone().open_entry(rig.store.clone(), entry.into(), None, 0, driver.clone());
            driver.drive(&task);
            // A task started after the card stopped answering waits at
            // PlugIn, as it should: give up on it after a while.
            if !ends_within(&task, Duration::from_secs(5)) {
                task.cancel();
            }
            prop_assert!(!task.join(), "the task's thread panicked");
        }
        let driver = Driver::helpful(PIN, &sim);
        let login = login(&port, LoginPurpose::Fetch, &driver);
        // `signer` waits for as long as the unlock takes: bound it.
        let watchdog = login.task().clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            watchdog.cancel();
        });
        if let Ok(signer) = login.signer() {
            let _ = paw_core::key::SshSigner::sign(&signer, b"login");
            login.finish();
        }
        login.task().cancel();
        let task = login.task().clone();
        drop(login);
        prop_assert!(!task.join(), "the sign-in's thread panicked");
    }
}

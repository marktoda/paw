//! Whatever a card (or something posing as one) answers, the key session
//! does not panic: it fails the task, or carries on.

#![cfg(feature = "sim")]

mod key_common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use key_common::*;
use paw_core::ApduTransport;
use paw_core::key::KeyPort;
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

impl ApduTransport for Tampered {
    fn transmit(&self, apdu: &[u8]) -> Result<Vec<u8>, String> {
        let real = self.sim.transmit(apdu);
        match self.script.lock().unwrap().next() {
            Some(Some(answer)) if answer == b"fail" => Err("reader failed".into()),
            Some(Some(answer)) => Ok(answer),
            _ => real,
        }
    }
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
        if port.attach(card).is_err() {
            return Ok(());
        }
        for entry in [ENTRY, FOREIGN_ENTRY] {
            let driver = Driver::helpful(PIN, &sim);
            let task = port.clone().open_entry(rig.store.clone(), entry.into(), None, 0, driver.clone());
            driver.drive(&task);
            // A task started after the card stopped answering waits at
            // PlugIn, as it should: give up on it after a while.
            if task.wait_final(Duration::from_secs(5)).is_none() {
                task.cancel();
            }
            prop_assert!(!task.join(), "the task's thread panicked");
        }
        let driver = Driver::helpful(PIN, &sim);
        let task = port.clone().sign_in(paw_core::key::LoginPurpose::Fetch, driver.clone());
        driver.drive(&task);
        // `login_signer` waits for as long as the unlock takes: bound it.
        let watchdog = task.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            watchdog.cancel();
        });
        if let Ok(signer) = task.login_signer() {
            let _ = paw_core::ssh::SshSigner::sign(&signer, b"login");
            task.finish_login();
        }
        task.cancel();
        prop_assert!(!task.join(), "the sign-in's thread panicked");
    }
}

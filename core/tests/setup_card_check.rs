//! Setup's per-call path that takes a card and a PIN (`SetupFlow::download`
//! and `test_decrypt`, in sim builds only: the Rust tests use it to set
//! stores up; the app uses key tasks) sends the PIN only to the store's
//! enrolled key and the key read at "Your key": another card is refused
//! before any VERIFY. The key tasks' own refusal is in `enrolment.rs`.
//!
//! The card presented is always the simulated key, which can open the
//! fixture store, so without the check each path would verify the PIN; the
//! enrolled key is recorded as some other key.

#![cfg(feature = "sim")]

mod support;

use std::path::Path;
use std::sync::Arc;

use paw_core::key::KeyIdentity;
use paw_core::key::enrolment::save_enrolment;
use paw_core::setup::touch::TouchExpectation;
use paw_core::setup::{SetupError, SetupEvents, SetupFlow, SetupState};
use paw_core::sim::SimulatedKey;
use support::GitServer;

const PIN: &str = "123456";

fn key() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), false, 1000)
}

/// Some key that is not the simulated one.
fn someone_else() -> KeyIdentity {
    KeyIdentity {
        ident: "0006:11112222".into(),
        decryption_fingerprint: Some("AB".repeat(20)),
        authentication_fingerprint: Some("CD".repeat(20)),
    }
}

fn says_different_key(text: &str) -> bool {
    text.contains("a different key from the one this store was set up with") && text.contains("serial 11112222")
}

struct Quiet;

impl SetupEvents for Quiet {
    fn touch_needed(&self, _: TouchExpectation) {}
    fn progress(&self, _: u32, _: u32) -> bool {
        true
    }
}

/// A setup at `NeedPin` against `server`, the key read.
fn at_need_pin(state: &Path, key: &Arc<SimulatedKey>, server: &GitServer) -> SetupFlow {
    let mut flow = SetupFlow::open(state).unwrap();
    flow.read_key(key.clone()).unwrap();
    flow.set_address(&server.sim.address()).unwrap();
    let SetupState::ConfirmHost { fingerprint, .. } = flow.check_host().unwrap() else { panic!() };
    flow.confirm_host(&fingerprint).unwrap();
    flow
}

#[test]
fn setups_download_and_test_decrypt_refuse_another_card_before_any_verify() {
    let key = key();
    let server = GitServer::start_with_fixture(&[key.ssh_public_key()]);
    let state = tempfile::tempdir().unwrap();
    let mut flow = at_need_pin(state.path(), &key, &server);

    // A store set up here before, with another key.
    save_enrolment(state.path(), &someone_else()).unwrap();
    let error = flow.download(key.clone(), PIN, Arc::new(Quiet)).unwrap_err();
    assert!(matches!(&error, SetupError::UnusableKey { message } if says_different_key(message)), "{error:?}");
    assert_eq!((key.verify_attempts(), server.logins()), (0, 0));
    assert!(matches!(flow.state(), SetupState::NeedPin { .. }));

    // Not the key read at "Your key".
    std::fs::remove_file(state.path().join("enrolled-key")).unwrap();
    let other = SimulatedKey::new_other(PIN.into(), false, 1000);
    let error = flow.download(other.clone(), PIN, Arc::new(Quiet)).unwrap_err();
    assert!(matches!(error, SetupError::UnusableKey { .. }), "{error:?}");
    assert_eq!(other.verify_attempts() + other.pin_queries(), 0);

    // The right key downloads; then the test decrypt refuses another.
    assert!(matches!(flow.download(key.clone(), PIN, Arc::new(Quiet)).unwrap(), SetupState::TestDecrypt { .. }));
    let verified = key.verify_attempts();
    save_enrolment(state.path(), &someone_else()).unwrap();
    let error = flow.test_decrypt(key.clone(), PIN, Arc::new(Quiet)).unwrap_err();
    assert!(matches!(&error, SetupError::UnusableKey { message } if says_different_key(message)), "{error:?}");
    assert_eq!(key.verify_attempts(), verified);
    assert_eq!(key.decipher_count(), 0);

}

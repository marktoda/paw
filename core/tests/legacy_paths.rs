//! The older paths that take a card and a PIN per call send the PIN only to
//! the store's enrolled key: another card is refused before any VERIFY.
//!
//! The card presented is always the simulated key, which can open the
//! fixture store, so without the check each path would verify the PIN; the
//! enrolled key is recorded as some other key.

#![cfg(feature = "sim")]

mod support;

use std::path::Path;
use std::sync::Arc;

use paw_core::PawError;
use paw_core::copies::CopyCache;
use paw_core::ffi::sync::{Setup, StoreSync};
use paw_core::key::KeyIdentity;
use paw_core::key::enrolment::save_enrolment;
use paw_core::setup::touch::TouchExpectation;
use paw_core::setup::{SetupError, SetupEvents, SetupFlow, SetupState};
use paw_core::sim::SimulatedKey;
use paw_core::sim::copies::{SimClock, SimKeystore};
use paw_core::sim::fixture::write_fixture_store;
use paw_core::store::PassStore;
use paw_core::sync::SyncError;
use support::GitServer;

const PIN: &str = "123456";
const ENTRY: &str = "web/github.com";

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

/// The fixture store checked out where setup puts it, `<state>/repo`.
fn set_up_store(state: &Path) -> Arc<PassStore> {
    let repo = state.join("repo");
    write_fixture_store(repo.to_string_lossy().into_owned()).unwrap();
    PassStore::open(repo.to_string_lossy().into_owned()).unwrap()
}

#[test]
fn open_entry_refuses_another_card_before_any_verify() {
    let key = key();
    let state = tempfile::tempdir().unwrap();
    let store = set_up_store(state.path());
    save_enrolment(state.path(), &someone_else()).unwrap();

    let error = store.open_entry(key.clone().as_connection(), PIN.into(), ENTRY.into()).err().unwrap();

    assert!(matches!(&error, PawError::NotForThisKey { recipients } if says_different_key(&recipients[0])), "{error:?}");
    assert_eq!(key.verify_attempts(), 0);
    assert_eq!(key.decipher_count(), 0);
}

#[test]
fn open_entry_keeping_copy_refuses_another_card_before_any_verify() {
    let key = key();
    let state = tempfile::tempdir().unwrap();
    let store = set_up_store(state.path());
    save_enrolment(state.path(), &someone_else()).unwrap();
    let window = tempfile::tempdir().unwrap();
    let copies = CopyCache::new(
        window.path().join("copies").to_string_lossy().into_owned(),
        SimKeystore::new(),
        SimClock::new(1_790_000_000_000),
    );

    let error = store
        .open_entry_keeping_copy(key.clone().as_connection(), PIN.into(), ENTRY.into(), copies, 3600)
        .err()
        .unwrap();

    assert!(matches!(&error, PawError::NotForThisKey { recipients } if says_different_key(&recipients[0])), "{error:?}");
    assert_eq!(key.verify_attempts(), 0);
}

#[test]
fn the_enrolled_key_and_stores_paw_did_not_set_up_still_open() {
    let key = key();
    // The enrolled key itself.
    let state = tempfile::tempdir().unwrap();
    let store = set_up_store(state.path());
    let info = paw_core::card_info(key.clone()).unwrap();
    save_enrolment(state.path(), &KeyIdentity {
        ident: info.ident,
        decryption_fingerprint: info.decryption_fingerprint,
        authentication_fingerprint: info.authentication_fingerprint,
    })
    .unwrap();
    assert!(store.open_entry(key.clone().as_connection(), PIN.into(), ENTRY.into()).is_ok());

    // A store opened anywhere but `<state>/repo` has no enrolled key.
    let elsewhere = tempfile::tempdir().unwrap();
    let fixture = elsewhere.path().join("fixture");
    write_fixture_store(fixture.to_string_lossy().into_owned()).unwrap();
    save_enrolment(elsewhere.path(), &someone_else()).unwrap();
    let store = PassStore::open(fixture.to_string_lossy().into_owned()).unwrap();
    assert!(store.open_entry(key.clone().as_connection(), PIN.into(), ENTRY.into()).is_ok());
    assert_eq!(key.verify_attempts(), 2);
}

#[test]
fn a_damaged_enrolment_record_refuses_every_card() {
    let key = key();
    let state = tempfile::tempdir().unwrap();
    let store = set_up_store(state.path());
    std::fs::write(state.path().join("enrolled-key"), "ident=x\n").unwrap();

    assert!(store.open_entry(key.clone().as_connection(), PIN.into(), ENTRY.into()).is_err());
    assert_eq!(key.verify_attempts(), 0);
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

    // The same through the exported one-shot `Setup`.
    let setup = Setup::open(state.path().to_string_lossy().into_owned()).unwrap();
    let error = setup.test_decrypt(key.clone().as_connection(), PIN.into()).unwrap_err();
    assert!(matches!(error, SetupError::UnusableKey { .. }), "{error:?}");
    assert_eq!(key.verify_attempts(), verified);
}

#[test]
fn store_syncs_send_and_refresh_refuse_another_card_before_any_verify() {
    let key = key();
    let server = GitServer::start_with_fixture(&[key.ssh_public_key()]);
    let state = tempfile::tempdir().unwrap();
    let mut flow = at_need_pin(state.path(), &key, &server);
    flow.download(key.clone(), PIN, Arc::new(Quiet)).unwrap();
    flow.test_decrypt(key.clone(), PIN, Arc::new(Quiet)).unwrap();
    let sync = StoreSync::open(state.path().to_string_lossy().into_owned()).unwrap();
    // It works with the enrolled key.
    sync.refresh(key.clone().as_connection(), PIN.into()).unwrap();
    let (verified, logins) = (key.verify_attempts(), server.logins());

    save_enrolment(state.path(), &someone_else()).unwrap();
    for error in [
        sync.send(key.clone().as_connection(), PIN.into()).unwrap_err(),
        sync.refresh(key.clone().as_connection(), PIN.into()).unwrap_err(),
    ] {
        assert!(matches!(&error, SyncError::Failed { message } if says_different_key(message)), "{error:?}");
    }
    assert_eq!((key.verify_attempts(), server.logins()), (verified, logins));
}

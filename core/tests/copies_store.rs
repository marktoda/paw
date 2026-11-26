//! Opening fixture-store entries the way the app will: with the key the
//! first time, by Face ID after that, and with the key again once the entry
//! changes.

#![cfg(feature = "sim")]

mod key_common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use key_common::{Driver, finish, is_touch};
use paw_core::PawError;
use paw_core::copies::{CopyCache, CopyStatus};
use paw_core::open::FaceIdOpen;
use paw_core::key::KeyPort;
use paw_core::task::{KeyOutcome, KeyTaskState};
use paw_core::sim::SimulatedKey;
use paw_core::sim::copies::{SimClock, SimKeystore};
use paw_core::sim::fixture::write_fixture_store;
use paw_core::store::PassStore;

const PIN: &str = "123456";
const ENTRY: &str = "web/github.com";
const EIGHT_HOURS: u32 = 8 * 60 * 60;

/// The simulated key plugged into the app's key port. `touches` counts the
/// times a task asked for a touch (and got one).
struct Plugged {
    key: Arc<SimulatedKey>,
    port: Arc<KeyPort>,
    touches: AtomicU32,
}

struct Phone {
    store_dir: tempfile::TempDir,
    store: Arc<PassStore>,
    key: Arc<Plugged>,
    keystore: Arc<SimKeystore>,
    clock: Arc<SimClock>,
    copies: Arc<CopyCache>,
    _window: tempfile::TempDir,
}

fn phone() -> Phone {
    let store_dir = tempfile::tempdir().unwrap();
    write_fixture_store(store_dir.path().to_str().unwrap().into()).unwrap();
    let store = PassStore::open(store_dir.path().to_str().unwrap().into()).unwrap();
    let window = tempfile::tempdir().unwrap();
    let keystore = SimKeystore::new();
    let clock = SimClock::new(1_790_000_000_000);
    let sim = SimulatedKey::new(PIN.into(), true, 5_000);
    let port = KeyPort::new(clock.clone());
    port.plugged_in(sim.clone()).unwrap();
    let key = Arc::new(Plugged { key: sim, port, touches: AtomicU32::new(0) });
    let copies = CopyCache::new(
        window.path().join("window").to_str().unwrap().into(),
        keystore.clone(),
        clock.clone(),
    );
    Phone { store_dir, store, key, keystore, clock, copies, _window: window }
}

impl Phone {
    /// Opens the entry as the app does, with a key task that keeps a Face
    /// ID copy for `ttl_secs`; types the PIN and touches when asked.
    fn open_with_key(&self, ttl_secs: u32) -> Option<i64> {
        let driver = Driver::helpful(PIN, &self.key.key);
        let task = self.key.port.clone().open_entry(
            self.store.clone(),
            ENTRY.into(),
            Some(self.copies.clone()),
            ttl_secs,
            driver.clone(),
        );
        driver.drive(&task);
        let KeyTaskState::Done { outcome: KeyOutcome::Opened { entry, face_id_until_ms, .. } } = finish(&task) else {
            panic!("the entry did not open: {:?}", driver.states());
        };
        self.key.touches.fetch_add(driver.count(is_touch) as u32, Ordering::SeqCst);
        assert_eq!(entry.password().as_deref(), Some("tidal-Quartz-88-ferns"));
        face_id_until_ms
    }

    fn open_with_face_id(&self) -> Result<FaceIdOpen, PawError> {
        self.store.open_entry_with_face_id(self.copies.clone(), ENTRY.into(), "Open GitHub".into())
    }
}

#[test]
fn open_with_key_then_face_id_costs_one_touch() {
    let p = phone();
    assert!(matches!(p.open_with_face_id().unwrap(), FaceIdOpen::NeedsKey));
    assert_eq!(p.keystore.prompt_count(), 0);

    let until = p.open_with_key(EIGHT_HOURS).unwrap();
    assert_eq!(
        p.store.face_id_status(p.copies.clone(), ENTRY.into()).unwrap(),
        CopyStatus::Available { until_ms: until }
    );
    for _ in 0..3 {
        let FaceIdOpen::Opened { entry } = p.open_with_face_id().unwrap() else {
            panic!("expected the copy to open");
        };
        assert_eq!(entry.username().as_deref(), Some("you@example.com"));
    }
    assert_eq!(p.key.key.decipher_count(), 1);
    assert_eq!(p.key.touches.load(Ordering::SeqCst), 1);
    assert_eq!(p.keystore.prompt_count(), 3);

    p.clock.advance_ms(u64::from(EIGHT_HOURS) * 1000);
    assert!(matches!(p.open_with_face_id().unwrap(), FaceIdOpen::NeedsKey));
}

#[test]
fn an_entry_changed_by_a_pull_needs_the_key_again() {
    let p = phone();
    p.open_with_key(EIGHT_HOURS);
    // Another device re-encrypted the entry; the pull wrote new bytes.
    let file = p.store_dir.path().join(format!("{ENTRY}.gpg"));
    let mut bytes = std::fs::read(&file).unwrap();
    bytes.push(0);
    std::fs::write(&file, bytes).unwrap();

    assert_eq!(p.store.face_id_status(p.copies.clone(), ENTRY.into()).unwrap(), CopyStatus::Stale);
    assert!(matches!(p.open_with_face_id().unwrap(), FaceIdOpen::NeedsKey));
    assert_eq!(p.keystore.prompt_count(), 0);
}

#[test]
fn copies_turned_off_keep_nothing() {
    let p = phone();
    assert_eq!(p.open_with_key(0), None);
    assert!(p.copies.list().unwrap().is_empty());
    assert!(matches!(p.open_with_face_id().unwrap(), FaceIdOpen::NeedsKey));
}

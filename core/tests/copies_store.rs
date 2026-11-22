//! Opening fixture-store entries the way the app will: with the key the
//! first time, by Face ID after that, and with the key again once the entry
//! changes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use paw_core::PawError;
use paw_core::copies::{CopyCache, CopyStatus};
use paw_core::ffi::{CardConnection, ConnectionError, FaceIdOpen};
use paw_core::sim::SimulatedKey;
use paw_core::sim::copies::{SimClock, SimKeystore};
use paw_core::sim::fixture::write_fixture_store;
use paw_core::store::PassStore;

const PIN: &str = "123456";
const ENTRY: &str = "web/github.com";
const EIGHT_HOURS: u32 = 8 * 60 * 60;

struct Plugged {
    key: Arc<SimulatedKey>,
    touches: AtomicU32,
}

impl CardConnection for Plugged {
    fn transmit(&self, apdu: Vec<u8>) -> Result<Vec<u8>, ConnectionError> {
        Ok(self.key.exchange(apdu))
    }

    fn touch_needed(&self) {
        self.touches.fetch_add(1, Ordering::SeqCst);
        self.key.touch();
    }
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
    let key = Arc::new(Plugged { key: SimulatedKey::new(PIN.into(), true, 50), touches: AtomicU32::new(0) });
    let window = tempfile::tempdir().unwrap();
    let keystore = SimKeystore::new();
    let clock = SimClock::new(1_790_000_000_000);
    let copies = CopyCache::new(
        window.path().join("window").to_str().unwrap().into(),
        keystore.clone(),
        clock.clone(),
    );
    Phone { store_dir, store, key, keystore, clock, copies, _window: window }
}

impl Phone {
    fn open_with_key(&self, ttl_secs: u32) -> Option<i64> {
        let opened = self
            .store
            .open_entry_keeping_copy(self.key.clone(), PIN.into(), ENTRY.into(), self.copies.clone(), ttl_secs)
            .unwrap();
        assert_eq!(opened.entry.password().as_deref(), Some("tidal-Quartz-88-ferns"));
        opened.face_id_until_ms
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

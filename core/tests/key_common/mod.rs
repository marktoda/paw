//! A plugged-in simulated key, a fixture store and a listener that records
//! every stage, for the key session tests.

// Each test binary uses its own part of this.
#![allow(dead_code)]

use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use paw_core::copies::CopyCache;
use paw_core::key::{KeyPort, KeyTask, KeyTaskListener, KeyTaskState};
use paw_core::sim::copies::{SimClock, SimKeystore};
use paw_core::sim::fixture::write_fixture_store;
use paw_core::sim::{SimOptions, SimTouch, SimulatedKey};
use paw_core::store::PassStore;

pub const PIN: &str = "735190";
/// The password line of [`ENTRY`] in the fixture store.
pub const SECRET: &str = "tidal-Quartz-88-ferns";
pub const ENTRY: &str = "web/github.com";
pub const OTHER_ENTRY: &str = "phone-pin";
/// Encrypted for a key the simulated card does not hold.
pub const FOREIGN_ENTRY: &str = "archive/old-forum";
pub const WAIT: Duration = Duration::from_secs(10);

pub struct Rig {
    pub sim: Arc<SimulatedKey>,
    pub clock: Arc<SimClock>,
    pub port: Arc<KeyPort>,
    pub store: Arc<PassStore>,
    pub copies: Arc<CopyCache>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

pub fn sim(decrypt: SimTouch, auth: SimTouch, touch_timeout: Duration) -> Arc<SimulatedKey> {
    let sim = SimulatedKey::with_options(SimOptions {
        pin: PIN.into(),
        decrypt_touch: false,
        auth_touch: false,
        touch_timeout,
        extended_length: false,
    });
    sim.set_decryption_touch(decrypt);
    sim.set_authentication_touch(auth);
    sim
}

/// A key whose slots need a touch for every use, not yet plugged in.
pub fn rig_unplugged(decrypt: SimTouch, auth: SimTouch) -> Rig {
    let store_dir = tempfile::tempdir().unwrap();
    write_fixture_store(store_dir.path().to_string_lossy().into_owned()).unwrap();
    let store = PassStore::open(store_dir.path().to_string_lossy().into_owned()).unwrap();
    let copies_dir = tempfile::tempdir().unwrap();
    let clock = SimClock::new(1_790_000_000_000);
    let copies = CopyCache::new(
        copies_dir.path().join("window").to_string_lossy().into_owned(),
        SimKeystore::new(),
        clock.clone(),
    );
    Rig {
        sim: sim(decrypt, auth, Duration::from_secs(5)),
        port: KeyPort::new(clock.clone()),
        clock,
        store,
        copies,
        _dirs: (store_dir, copies_dir),
    }
}

pub fn rig(decrypt: SimTouch, auth: SimTouch) -> Rig {
    let rig = rig_unplugged(decrypt, auth);
    rig.port.attach(rig.sim.clone()).unwrap();
    rig
}

/// Records every stage; can type the PIN and touch the key when asked.
#[derive(Default)]
pub struct Driver {
    states: Mutex<Vec<KeyTaskState>>,
    changed: Condvar,
    task: OnceLock<Weak<KeyTask>>,
    pin: Option<String>,
    toucher: Option<Arc<SimulatedKey>>,
}

impl Driver {
    /// Watches only.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Types `pin` at the PIN stage and touches `sim` at the touch stage.
    pub fn helpful(pin: &str, sim: &Arc<SimulatedKey>) -> Arc<Self> {
        Arc::new(Self { pin: Some(pin.into()), toucher: Some(sim.clone()), ..Self::default() })
    }

    /// Touches `sim` when asked, but types nothing.
    pub fn toucher(sim: &Arc<SimulatedKey>) -> Arc<Self> {
        Arc::new(Self { toucher: Some(sim.clone()), ..Self::default() })
    }

    /// Lets the driver type the PIN, including at a PIN stage the task
    /// started in (which the listener is not told about).
    pub fn drive(&self, task: &Arc<KeyTask>) {
        let _ = self.task.set(Arc::downgrade(task));
        if let (Some(pin), KeyTaskState::Pin { .. }) = (&self.pin, task.state()) {
            task.supply_pin(pin.as_bytes().to_vec());
        }
    }

    pub fn states(&self) -> Vec<KeyTaskState> {
        self.states.lock().unwrap().clone()
    }

    pub fn saw(&self, wanted: impl Fn(&KeyTaskState) -> bool) -> bool {
        self.states().iter().any(wanted)
    }

    pub fn count(&self, wanted: impl Fn(&KeyTaskState) -> bool) -> usize {
        self.states().iter().filter(|s| wanted(s)).count()
    }

    /// Waits until a stage `wanted` has been reported.
    pub fn wait_for(&self, wanted: impl Fn(&KeyTaskState) -> bool) {
        let deadline = Instant::now() + WAIT;
        let mut states = self.states.lock().unwrap();
        while !states.iter().any(&wanted) {
            let left = deadline.checked_duration_since(Instant::now()).expect("the stage never came");
            states = self.changed.wait_timeout(states, left).unwrap().0;
        }
    }
}

impl KeyTaskListener for Driver {
    fn state_changed(&self, state: KeyTaskState) {
        match &state {
            KeyTaskState::Pin { .. } => {
                if let (Some(pin), Some(task)) = (&self.pin, self.task.get().and_then(Weak::upgrade)) {
                    task.supply_pin(pin.as_bytes().to_vec());
                }
            }
            KeyTaskState::Touch => {
                if let Some(sim) = &self.toucher {
                    sim.touch();
                }
            }
            _ => {}
        }
        self.states.lock().unwrap().push(state);
        self.changed.notify_all();
    }
}

pub fn finish(task: &Arc<KeyTask>) -> KeyTaskState {
    task.wait_final(WAIT).expect("the task never finished")
}

pub fn is_pin(state: &KeyTaskState) -> bool {
    matches!(state, KeyTaskState::Pin { .. })
}

pub fn is_touch(state: &KeyTaskState) -> bool {
    matches!(state, KeyTaskState::Touch)
}

pub fn is_busy(state: &KeyTaskState) -> bool {
    matches!(state, KeyTaskState::Busy)
}

pub fn is_done(state: &KeyTaskState) -> bool {
    matches!(state, KeyTaskState::Done { .. })
}

/// Starts opening `entry`, with no Face ID copy, driven by `driver`.
pub fn open(rig: &Rig, entry: &str, driver: &Arc<Driver>) -> Arc<KeyTask> {
    let task = rig.port.clone().open_entry(rig.store.clone(), entry.into(), None, 0, driver.clone());
    driver.drive(&task);
    task
}

/// Waits until `check` holds, for things no listener reports.
pub fn eventually(check: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

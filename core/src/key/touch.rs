//! When to show `Touch`.
//!
//! Only once the command that waits for the touch has gone out to the card
//! (so a touch cannot release some other, perhaps cancelled, command), and
//! only if the slot's policy can ask for one:
//!
//! - policy off: never;
//! - a touch for every use: as the command goes out;
//! - cached, or a policy paw does not know: only if the card is still
//!   waiting [`CACHED_TOUCH_GRACE`] later, because it may answer at once
//!   from an earlier touch and a prompt that flashes teaches people to
//!   ignore it.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use super::task::TaskCore;
use super::wire::{TouchHook, Wire};
use super::{CACHED_TOUCH_GRACE, KeyTaskState, KeyTouch};

/// Reports `Touch` for the commands sent while it lives.
pub(crate) struct TouchWatch {
    wire: Arc<Wire>,
}

impl TouchWatch {
    /// `slot` names the slot in the key log ("decryption", "authentication").
    pub(crate) fn arm(wire: &Arc<Wire>, task: &Arc<TaskCore>, policy: KeyTouch, slot: &str) -> Self {
        let watch = match policy {
            KeyTouch::Off => "none",
            KeyTouch::EachUse => "at once",
            KeyTouch::Cached | KeyTouch::Unknown => "delayed",
        };
        wire.log().line(format!(
            "{} touch watch {watch} ({slot} slot: {})",
            task.tag(),
            super::keylog::touch_words(policy)
        ));
        let hook: Option<Arc<dyn TouchHook>> = match policy {
            KeyTouch::Off => None,
            KeyTouch::EachUse => Some(Arc::new(AtOnce { task: task.clone() })),
            KeyTouch::Cached | KeyTouch::Unknown => Some(Arc::new(Delayed {
                task: task.clone(),
                shared: Arc::new(Shared::default()),
            })),
        };
        wire.set_touch_hook(hook);
        Self { wire: wire.clone() }
    }
}

impl Drop for TouchWatch {
    fn drop(&mut self) {
        self.wire.set_touch_hook(None);
    }
}

/// Whether a policy can make the card wait for a touch.
pub(crate) fn possible(policy: KeyTouch) -> bool {
    policy != KeyTouch::Off
}

fn working() -> KeyTaskState {
    KeyTaskState::Working { progress: None }
}

struct AtOnce {
    task: Arc<TaskCore>,
}

impl TouchHook for AtOnce {
    fn sent(&self) {
        self.task.set(KeyTaskState::Touch);
    }

    fn answered(&self) {
        self.task.set(working());
    }
}

#[derive(Default)]
struct Pending {
    /// Counts commands, so a timer left over from an earlier one does
    /// nothing.
    command: u64,
    waiting: bool,
    shown: bool,
}

#[derive(Default)]
struct Shared {
    pending: Mutex<Pending>,
    answered: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Pending> {
        // Plain values only.
        self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

struct Delayed {
    task: Arc<TaskCore>,
    shared: Arc<Shared>,
}

impl TouchHook for Delayed {
    fn sent(&self) {
        let command = {
            let mut pending = self.shared.lock();
            pending.command += 1;
            pending.waiting = true;
            pending.command
        };
        let (shared, task) = (self.shared.clone(), self.task.clone());
        // If no thread can be started, the prompt is not shown: the card
        // still waits, and the sheet says the key is working.
        let _ = std::thread::Builder::new().name("paw-touch-grace".into()).spawn(move || {
            let pending = shared.lock();
            let (mut pending, _) = shared
                .answered
                .wait_timeout_while(pending, CACHED_TOUCH_GRACE, |p| p.waiting && p.command == command)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if pending.waiting && pending.command == command {
                pending.shown = true;
                // Under the lock, so `Touch` is queued before the `Working`
                // that the answer brings.
                task.set(KeyTaskState::Touch);
            }
        });
    }

    fn answered(&self) {
        let shown = {
            let mut pending = self.shared.lock();
            pending.waiting = false;
            std::mem::take(&mut pending.shown)
        };
        self.shared.answered.notify_all();
        if shown {
            self.task.set(working());
        }
    }
}

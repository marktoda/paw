//! The key session as Swift sees it. The types and most methods are
//! exported where they are defined, in [`crate::key`]; this adds what takes
//! the Swift card connection and clock.

use std::sync::Arc;

use super::{Adapter, CardConnection};
use crate::copies::Clock;
use crate::key::{KeyError, KeyPort, KeySession, Uptime};

struct ClockUptime(Arc<dyn Clock>);

impl Uptime for ClockUptime {
    fn monotonic_ms(&self) -> i64 {
        self.0.monotonic_ms()
    }
}

#[uniffi::export]
impl KeyPort {
    /// `clock` measures time in the background for the PIN limit (its
    /// monotonic clock).
    #[uniffi::constructor]
    pub fn new(clock: Arc<dyn Clock>) -> Arc<Self> {
        Self::with_uptime(Arc::new(ClockUptime(clock)))
    }

    /// A key was plugged in. Reads it (no PIN, no touch) and makes its
    /// session; a task waiting at `PlugIn` goes on.
    pub fn plugged_in(&self, connection: Arc<dyn CardConnection>) -> Result<Arc<KeySession>, KeyError> {
        self.attach(Arc::new(Adapter(connection)))
    }
}

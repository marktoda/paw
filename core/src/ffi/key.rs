//! The key session as Swift sees it. The types and most methods are
//! exported where they are defined, in [`crate::key`]; this adds what takes
//! the Swift card connection.

use std::sync::Arc;

use super::{Adapter, CardConnection};
use crate::key::{KeyError, KeyPort, KeySession};

#[uniffi::export]
impl KeyPort {
    /// A key was plugged in. Reads it (no PIN, no touch) and makes its
    /// session; a task waiting at `PlugIn` goes on. The connection's
    /// `touch_needed` is not called: tasks report `Touch` themselves.
    pub fn plugged_in(&self, connection: Arc<dyn CardConnection>) -> Result<Arc<KeySession>, KeyError> {
        self.attach(Arc::new(Adapter(connection)))
    }
}

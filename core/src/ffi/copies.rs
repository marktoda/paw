//! Opening an entry with Face ID copies in the picture.
//!
//! The plaintext never crosses to Swift to be stored: a key open (a key
//! task, `KeyPort::open_entry`) keeps its copy in the core, and a Face ID
//! open hands Swift an [`OpenedEntry`], exactly as a key open does.

use std::sync::Arc;

use crate::entry::opened::OpenedEntry;
use crate::PawError;
use crate::copies::{CopyCache, CopyStatus};
use crate::files::content_id;
use crate::store::PassStore;

/// What came of trying Face ID first.
#[derive(uniffi::Enum)]
pub enum FaceIdOpen {
    Opened { entry: Arc<OpenedEntry> },
    /// There is no live copy: ask for the key. Nothing was prompted.
    NeedsKey,
}

#[uniffi::export]
impl PassStore {
    /// Opens `entry` from its Face ID copy if it has a live one, showing
    /// `reason` in the prompt. Returns `NeedsKey`, without prompting, if it
    /// has none or the entry changed since the copy was made.
    pub fn open_entry_with_face_id(
        &self,
        copies: Arc<CopyCache>,
        entry: String,
        reason: String,
    ) -> Result<FaceIdOpen, PawError> {
        let current = content_id(&self.read_entry(&entry)?);
        match copies.open(&entry, &current, &reason) {
            Ok(mut plaintext) => Ok(FaceIdOpen::Opened {
                entry: OpenedEntry::decrypted(std::mem::take(&mut *plaintext), current)?,
            }),
            Err(PawError::CopyUnavailable) => Ok(FaceIdOpen::NeedsKey),
            Err(e) => Err(e),
        }
    }

    /// Whether `entry` can be opened by Face ID now, against its current
    /// file. Never prompts.
    pub fn face_id_status(&self, copies: Arc<CopyCache>, entry: String) -> Result<CopyStatus, PawError> {
        let current = content_id(&self.read_entry(&entry)?);
        copies.status(&entry, &current)
    }
}

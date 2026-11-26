//! Opening an entry: with the key (a key task, `KeyPort::open_entry`) or
//! from its Face ID copy. Both end here, in an [`OpenedEntry`] made the
//! same way: decrypted bytes that are not text are refused
//! ([`PawError::NotText`]) wherever they came from, and the content id is
//! that of the file the entry was read from.
//!
//! The plaintext never crosses to Swift to be stored: a key open keeps its
//! Face ID copy in the core ([`keep_copy`]), and a Face ID open hands Swift
//! an [`OpenedEntry`], exactly as a key open does.

use std::sync::Arc;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::PawError;
use crate::copies::{CopyCache, CopyStatus};
use crate::entry::opened::OpenedEntry;
use crate::files::content_id;
use crate::store::PassStore;

/// `plaintext`, decrypted from the file whose content id is `content_id`,
/// as an open entry.
pub(crate) fn opened(mut plaintext: Zeroizing<Vec<u8>>, content_id: Vec<u8>) -> Result<Arc<OpenedEntry>, PawError> {
    OpenedEntry::decrypted(std::mem::take(&mut *plaintext), content_id)
}

/// Keeps a Face ID copy of `opened`, the entry named `name`, for `ttl`.
/// `Ok` with when it ends (wall clock), or `None` if the cache kept none.
pub(crate) fn keep_copy(
    copies: &CopyCache,
    name: &str,
    opened: &OpenedEntry,
    ttl: Duration,
) -> Result<Option<i64>, PawError> {
    let (Some(text), Some(id)) = (opened.read(|doc| doc.to_bytes()), opened.content_id()) else {
        return Err(PawError::CopyUnavailable);
    };
    copies.put(name, &text, id, ttl)
}

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
            Ok(plaintext) => Ok(FaceIdOpen::Opened { entry: opened(plaintext, current)? }),
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

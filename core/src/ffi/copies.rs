//! Opening an entry with Face ID copies in the picture.
//!
//! The plaintext never crosses to Swift to be stored: a key open keeps its
//! copy here, and a Face ID open hands Swift an [`OpenedEntry`], exactly as
//! a key open does.

use std::sync::Arc;
use std::time::Duration;

use super::{Adapter, CardConnection, OpenedEntry};
use crate::PawError;
use crate::copies::{CopyCache, CopyStatus, content_id};
use crate::store::PassStore;

/// An entry opened with the key.
#[derive(uniffi::Record)]
pub struct KeyOpen {
    pub entry: Arc<OpenedEntry>,
    /// Until when it can be opened again by Face ID (wall clock,
    /// milliseconds since 1970), for "Open with Face ID until 02:35".
    /// `None` if no copy was kept: copies are off for it, or keeping one
    /// failed.
    pub face_id_until_ms: Option<i64>,
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
    /// Opens `entry` with the key, as `open_entry` does, then keeps a Face
    /// ID copy of it for `ttl_secs` (capped at 12 hours; zero keeps none
    /// and forgets an older one).
    ///
    /// The entry opens even if keeping the copy fails: the person has
    /// already touched the key. `face_id_until_ms` is then `None`.
    pub fn open_entry_keeping_copy(
        &self,
        connection: Arc<dyn CardConnection>,
        pin: String,
        entry: String,
        copies: Arc<CopyCache>,
        ttl_secs: u32,
    ) -> Result<KeyOpen, PawError> {
        let ciphertext = self.read_entry(&entry)?;
        let touch = connection.clone();
        let mut plaintext = zeroize::Zeroizing::new(crate::decrypt_entry(
            Arc::new(Adapter(connection)),
            &pin,
            &ciphertext,
            &move || touch.touch_needed(),
        )?);
        let ttl = Duration::from_secs(ttl_secs.into());
        let face_id_until_ms =
            copies.put(&entry, &plaintext, &content_id(&ciphertext), ttl).ok().flatten();
        let entry = OpenedEntry::from_plaintext(std::mem::take(&mut *plaintext))?;
        Ok(KeyOpen { entry, face_id_until_ms })
    }

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
                entry: OpenedEntry::from_plaintext(std::mem::take(&mut *plaintext))?,
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

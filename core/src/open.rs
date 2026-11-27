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

use crate::PawError;
use crate::copies::{CopyCache, CopyStatus};
use crate::entry::opened::OpenedEntry;
use crate::files::content_id;
use crate::store::PassStore;
use crate::store::service::{self, UrlCheck};

/// Keeps a Face ID copy of `opened` for `ttl`. `Ok` with when it ends
/// (wall clock), or `None` if the cache kept none.
pub(crate) fn keep_copy(copies: &CopyCache, opened: &OpenedEntry, ttl: Duration) -> Result<Option<i64>, PawError> {
    let Some(text) = opened.read(|doc| doc.to_bytes()) else {
        return Err(PawError::CopyUnavailable);
    };
    copies.put(opened.path(), &text, opened.content_id(), ttl)
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
            Ok(plaintext) => Ok(FaceIdOpen::Opened { entry: OpenedEntry::decrypted(&entry, plaintext, current)? }),
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

/// Checks an opened entry's own address fields ([`EntryDoc::addresses`])
/// against the sites being filled, for AutoFill before it hands the
/// password over. Reads the fields only; keeps nothing.
///
/// [`EntryDoc::addresses`]: crate::entry::EntryDoc::addresses
#[uniffi::export]
pub fn check_entry_url(entry: Arc<OpenedEntry>, services: Vec<String>) -> UrlCheck {
    entry.read(|doc| service::check_urls(&doc.addresses(), &services)).unwrap_or(UrlCheck::NoUrl)
}

/// The user name AutoFill fills for `entry`: its own
/// ([`EntryDoc::filled_username`]), else its path's guess
/// ([`service::path_user`]).
///
/// [`EntryDoc::filled_username`]: crate::entry::EntryDoc::filled_username
#[uniffi::export]
pub fn fill_username(entry: Arc<OpenedEntry>) -> Option<String> {
    entry.read(|doc| doc.filled_username().map(str::to_owned)).flatten().or_else(|| service::path_user(entry.path()))
}

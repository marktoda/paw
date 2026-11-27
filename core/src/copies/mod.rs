//! Face ID copies: an entry opened with the key stays readable by Face ID
//! for a while, measured from that entry's own key touch.
//!
//! This is the one deliberate weakening of "every read needs the key"
//! (design.md 5.4), so the rules are kept here, in one place, and enforced
//! whenever a copy is read rather than by a timer that might not run:
//!
//! - A copy ends at its deadline, or once its length has passed on the
//!   monotonic clock, whichever comes first. Face ID never extends it.
//! - A copy made from other ciphertext than the entry's current file is
//!   stale and ends.
//! - If the wall clock is behind when any copy was made, or the phone has
//!   rebooted since, every copy ends and the keystore is reset.
//! - A copy whose sealed metadata does not match its file name, or that
//!   does not authenticate, is wiped.
//!
//! Every call first reads every copy's metadata (a handful of small files)
//! and applies these rules, wiping what has ended, so an ended copy is
//! removed the next time either process touches the cache. All of that
//! happens before any Face ID prompt.

mod dir;
pub mod format;
mod seams;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::PawError;
use format::{CopyFile, IndexKeys, Invalid, Meta};
pub use seams::{DeviceKeystore, KeystoreError};

use crate::clock::Clock;
use crate::files::remove_if_there;

/// No copy lives longer, whatever the caller asks for.
pub const MAX_TTL: Duration = Duration::from_secs(12 * 60 * 60);
/// In the copies folder while the person has turned Face ID copies off
/// (Settings). Not a `.copy` file, so ending every copy leaves it.
pub const OFF_MARKER: &str = "copies-off";

/// Whether an entry can be opened by Face ID now. Asking never prompts.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CopyStatus {
    /// A live copy, readable by Face ID until `until_ms` (wall clock,
    /// milliseconds since 1970).
    Available { until_ms: i64 },
    /// No live copy: open it with the key.
    NeedsKey,
    /// There was a copy, but the entry has changed since (a pull or an
    /// edit elsewhere). It has been wiped; open it with the key.
    Stale,
}

/// An entry with a live copy, for Home's "Open with Face ID" section.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LiveCopy {
    pub entry: String,
    /// When the copy ends (wall clock, milliseconds since 1970).
    pub until_ms: i64,
}

impl From<Invalid> for PawError {
    fn from(_: Invalid) -> Self {
        PawError::CopyFailed("a Face ID copy could not be sealed".into())
    }
}

fn io_err(e: std::io::Error) -> PawError {
    PawError::CopyFailed(format!("could not use the Face ID copies folder: {e}"))
}

fn keystore_err(e: KeystoreError) -> PawError {
    match e {
        KeystoreError::Cancelled => PawError::FaceIdCancelled,
        KeystoreError::NotRecognised => PawError::FaceIdNotRecognised,
        KeystoreError::LockedOut => PawError::FaceIdLockedOut,
        KeystoreError::NotEnrolled => PawError::FaceIdNotSetUp,
        KeystoreError::KeyGone => PawError::FaceIdChanged,
        KeystoreError::Failed { message } => PawError::CopyFailed(message),
    }
}

struct Now {
    wall_ms: i64,
    monotonic_ms: i64,
    boot_id: String,
}

impl Now {
    /// Whether `meta` shows the clock was set back or the phone rebooted,
    /// which ends every copy, not just this one.
    fn ends_all(&self, meta: &Meta) -> bool {
        meta.boot_id != self.boot_id
            || self.wall_ms < meta.created_wall_ms
            || self.monotonic_ms < meta.created_monotonic_ms
    }

    /// When the copy ends, or `None` if it has. The earlier of its wall
    /// deadline and its length on the monotonic clock, so moving the wall
    /// clock cannot lengthen it.
    fn until(&self, meta: &Meta) -> Option<i64> {
        let ttl = meta.ttl_ms();
        if ttl <= 0 || ttl > MAX_TTL.as_millis() as i64 {
            return None;
        }
        let elapsed = self.monotonic_ms.saturating_sub(meta.created_monotonic_ms);
        let left = meta.deadline_wall_ms.saturating_sub(self.wall_ms).min(ttl - elapsed);
        (left > 0).then(|| self.wall_ms + left)
    }
}

/// A copy that was live when the cache was last read.
struct Found {
    path: PathBuf,
    file: CopyFile,
    until_ms: i64,
}

/// The Face ID copies in one directory (`window/` in the App Group
/// container). The app and the extension each make their own on the same
/// directory; nothing is cached between calls.
#[derive(uniffi::Object)]
pub struct CopyCache {
    dir: PathBuf,
    keystore: Arc<dyn DeviceKeystore>,
    clock: Arc<dyn Clock>,
}

#[uniffi::export]
impl CopyCache {
    /// The copies in `dir`, created on the first copy if need be.
    #[uniffi::constructor]
    pub fn new(dir: String, keystore: Arc<dyn DeviceKeystore>, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self { dir: PathBuf::from(dir), keystore, clock })
    }

    /// Every entry with a live copy, soonest to end first. Never prompts.
    pub fn list(&self) -> Result<Vec<LiveCopy>, PawError> {
        let keys = self.keys()?;
        let mut live: Vec<LiveCopy> = self
            .scan(&keys)?
            .into_iter()
            .map(|found| LiveCopy { entry: found.file.meta.entry, until_ms: found.until_ms })
            .collect();
        live.sort_by(|a, b| (a.until_ms, &a.entry).cmp(&(b.until_ms, &b.entry)));
        Ok(live)
    }

    /// Wipes every copy that has ended. Every other call does this too;
    /// call it on launch and on each return to the foreground.
    pub fn sweep(&self) -> Result<(), PawError> {
        let keys = self.keys()?;
        self.scan(&keys)?;
        dir::remove_stale_temps(&self.dir).map_err(io_err)
    }

    /// Lock now: destroys the keystore's keys, so no copy can ever be
    /// opened again, then deletes every copy.
    pub fn lock_now(&self) -> Result<(), PawError> {
        self.end_all()
    }

    /// Whether Face ID copies are on: the default, until the person turns
    /// them off in Settings. Both processes read the same folder.
    pub fn copies_enabled(&self) -> bool {
        !self.off()
    }

    /// Settings › Face ID. Off ends every copy now, as Lock now does, and
    /// no copy is kept or opened after that: every open needs the key. On
    /// lets the next key open keep one again. Persisted beside the copies,
    /// so the AutoFill extension honours it with no change of its own.
    ///
    /// The setting is the marker file's presence: absent means on, as the
    /// app has always written it. Off flushes it to disk, so a power loss
    /// cannot turn copies back on.
    pub fn set_copies_enabled(&self, enabled: bool) -> Result<(), PawError> {
        let marker = self.dir.join(OFF_MARKER);
        if enabled {
            // A `put` that saw "on" just before copies went off may have
            // sealed under the keys reset then, and written its copy since.
            // While the marker is there nothing reads it; wipe it before the
            // marker goes, or it would be live again.
            dir::wipe(&self.dir).map_err(io_err)?;
            return remove_if_there(&marker).map_err(io_err);
        }
        // The marker first: a copy another process seals in between is
        // wiped by its next read and was sealed under keys reset below. If
        // the marker cannot be written, every copy still ends.
        let marked = dir::write_marker(&self.dir, OFF_MARKER).map_err(io_err);
        let ended = self.end_all();
        marked.and(ended)
    }

    /// Removes `entry`'s copy, if it has one, and no other: "Lock now" on
    /// an entry's screen, an open that was cancelled, an entry deleted or
    /// moved. Never prompts and never resets the keystore. Copies that have
    /// ended are wiped too, as by every call.
    pub fn forget_entry(&self, entry: String) -> Result<(), PawError> {
        self.scan(&self.keys()?)?;
        let keys = self.keys()?;
        remove_if_there(&self.path(&keys, &entry)).map_err(io_err)
    }
}

impl CopyCache {
    fn off(&self) -> bool {
        self.dir.join(OFF_MARKER).exists()
    }

    fn now(&self) -> Now {
        Now {
            wall_ms: self.clock.wall_ms(),
            monotonic_ms: self.clock.monotonic_ms(),
            boot_id: self.clock.boot_id(),
        }
    }

    fn keys(&self) -> Result<IndexKeys, PawError> {
        let index_key = Zeroizing::new(self.keystore.index_key().map_err(keystore_err)?);
        IndexKeys::derive(&index_key)
            .map_err(|_| PawError::CopyFailed("the keystore gave an index key of the wrong size".into()))
    }

    fn path(&self, keys: &IndexKeys, entry: &str) -> PathBuf {
        self.dir.join(format!("{}{}", keys.file_stem(entry), format::SUFFIX))
    }

    /// Ends every copy: the keys first, so nothing that survives the
    /// delete (a file another process is writing right now) can be opened.
    fn end_all(&self) -> Result<(), PawError> {
        let reset = self.keystore.reset();
        dir::wipe(&self.dir).map_err(io_err)?;
        reset.map_err(keystore_err)
    }

    /// Reads every copy, applies the rules in the module comment, and
    /// returns the copies still live. The clocks are read after the files,
    /// so a copy another process has just written is never "from the
    /// future".
    fn scan(&self, keys: &IndexKeys) -> Result<Vec<Found>, PawError> {
        // Turned off: nothing is live, and anything written since goes.
        if self.off() {
            dir::wipe(&self.dir).map_err(io_err)?;
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        for (stem, path) in dir::copy_files(&self.dir).map_err(io_err)? {
            let Some(bytes) = dir::read(&path).map_err(io_err)? else { continue };
            match CopyFile::parse(keys, &bytes) {
                Ok(file) if keys.file_stem(&file.meta.entry) == stem => files.push((path, file)),
                // Not ours, damaged, sealed under keys since reset, or
                // moved from another entry's name.
                _ => remove_if_there(&path).map_err(io_err)?,
            }
        }

        let now = self.now();
        if files.iter().any(|(_, file)| now.ends_all(&file.meta)) {
            self.end_all()?;
            return Ok(Vec::new());
        }
        let mut live = Vec::new();
        for (path, file) in files {
            match now.until(&file.meta) {
                Some(until_ms) => live.push(Found { path, file, until_ms }),
                None => remove_if_there(&path).map_err(io_err)?,
            }
        }
        Ok(live)
    }

    /// The live copy of `entry`, if any, with every other copy checked.
    fn find(&self, keys: &IndexKeys, entry: &str) -> Result<Option<Found>, PawError> {
        let path = self.path(keys, entry);
        Ok(self.scan(keys)?.into_iter().find(|found| found.path == path))
    }

    /// Keeps a copy of `text`, just decrypted with the key, for `ttl` from
    /// now (at most [`MAX_TTL`]). `content_id` identifies the ciphertext it
    /// came from (see [`crate::files::content_id`]). A zero `ttl` keeps nothing and
    /// forgets any older copy. Never prompts. Returns when the copy ends.
    pub fn put(
        &self,
        entry: &str,
        text: &[u8],
        content_id: &[u8],
        ttl: Duration,
    ) -> Result<Option<i64>, PawError> {
        // Applies the end-all rules before anything new is added. Ending
        // everything (a reboot, a clock set back) resets the keystore, so
        // the index keys are derived again afterwards: a copy written under
        // the old ones would be unreadable, and lost, at once.
        self.scan(&self.keys()?)?;
        let keys = self.keys()?;
        if ttl.is_zero() || self.off() {
            remove_if_there(&self.path(&keys, entry)).map_err(io_err)?;
            return Ok(None);
        }
        let ttl_ms = ttl.min(MAX_TTL).as_millis() as i64;
        let now = self.now();
        let meta = Meta {
            entry: entry.to_owned(),
            content_id: content_id.to_vec(),
            created_wall_ms: now.wall_ms,
            created_monotonic_ms: now.monotonic_ms,
            deadline_wall_ms: now.wall_ms.saturating_add(ttl_ms),
            boot_id: now.boot_id,
        };
        let deadline = meta.deadline_wall_ms;
        let sealed =
            CopyFile::seal(meta, text, |data_key| self.keystore.seal(data_key.to_vec()).map_err(Sealing::Keystore));
        let file = match sealed {
            Ok(file) => file,
            Err(Sealing::Invalid) => return Err(Invalid.into()),
            Err(Sealing::Keystore(e)) => return self.after_keystore_error(e, true),
        };
        dir::write_atomic(&self.dir, &keys.file_stem(entry), &file.to_bytes(&keys)?)
            .map_err(io_err)?;
        Ok(Some(deadline))
    }

    /// Whether `entry`, whose current ciphertext has `content_id`, can be
    /// opened by Face ID. Never prompts. A stale copy is wiped.
    pub fn status(&self, entry: &str, content_id: &[u8]) -> Result<CopyStatus, PawError> {
        let keys = self.keys()?;
        let Some(found) = self.find(&keys, entry)? else { return Ok(CopyStatus::NeedsKey) };
        if found.file.meta.content_id != content_id {
            remove_if_there(&found.path).map_err(io_err)?;
            return Ok(CopyStatus::Stale);
        }
        Ok(CopyStatus::Available { until_ms: found.until_ms })
    }

    /// Opens `entry`'s copy with Face ID, showing `reason`. A copy that has
    /// ended or is stale is wiped and refused before any prompt, with
    /// [`PawError::CopyUnavailable`]. A cancelled or failed Face ID leaves
    /// the copy as it is.
    pub fn open(
        &self,
        entry: &str,
        content_id: &[u8],
        reason: &str,
    ) -> Result<Zeroizing<Vec<u8>>, PawError> {
        let keys = self.keys()?;
        let Some(found) = self.find(&keys, entry)? else { return Err(PawError::CopyUnavailable) };
        if found.file.meta.content_id != content_id {
            remove_if_there(&found.path).map_err(io_err)?;
            return Err(PawError::CopyUnavailable);
        }

        let data_key = self
            .keystore
            .unseal(found.file.wrapped_key.clone(), reason.to_owned())
            .map(Zeroizing::new)
            .or_else(|e| {
                let still_there = found.path.exists();
                self.after_keystore_error(e, still_there)
            })?;

        // Face ID can take a while. Check again with the clock as it is now,
        // and that nobody ended or replaced the copy meanwhile.
        let unchanged = dir::read(&found.path)
            .map_err(io_err)?
            .and_then(|bytes| CopyFile::parse(&keys, &bytes).ok())
            .is_some_and(|file| file.meta == found.file.meta && file.wrapped_key == found.file.wrapped_key);
        let now = self.now();
        if now.ends_all(&found.file.meta) {
            self.end_all()?;
            return Err(PawError::CopyUnavailable);
        }
        if !unchanged || now.until(&found.file.meta).is_none() {
            remove_if_there(&found.path).map_err(io_err)?;
            return Err(PawError::CopyUnavailable);
        }
        found.file.open_text(&data_key).or_else(|_| {
            remove_if_there(&found.path).map_err(io_err)?;
            Err(PawError::CopyUnavailable)
        })
    }

    /// A key that is gone can never open anything again: end every copy.
    /// Says "Face ID changed" only if the copy being used was still there;
    /// if it had gone, Lock now ran in the other process.
    fn after_keystore_error<T>(&self, e: KeystoreError, copy_was_there: bool) -> Result<T, PawError> {
        if e != KeystoreError::KeyGone {
            return Err(keystore_err(e));
        }
        self.end_all()?;
        Err(if copy_was_there { PawError::FaceIdChanged } else { PawError::CopyUnavailable })
    }

}

enum Sealing {
    Keystore(KeystoreError),
    Invalid,
}

impl From<Invalid> for Sealing {
    fn from(_: Invalid) -> Self {
        Self::Invalid
    }
}

impl CopyCache {
    /// After paw saved `entry` (possibly under a new name, `to`): if `from`
    /// had a live copy, seals `text` (the saved plaintext) as the copy of
    /// `to` for what was left of the old one, and removes the old one. An
    /// edit never lengthens a copy: it still ends when the key touch that
    /// made it says. Returns when the copy ends, or `None` if there was
    /// none. Never prompts.
    pub fn reseal(
        &self,
        from: &str,
        to: &str,
        text: &[u8],
        content_id: &[u8],
    ) -> Result<Option<i64>, PawError> {
        let keys = self.keys()?;
        let Some(found) = self.find(&keys, from)? else { return Ok(None) };
        let left = found.until_ms.saturating_sub(self.now().wall_ms);
        remove_if_there(&found.path).map_err(io_err)?;
        if left <= 0 {
            return Ok(None);
        }
        self.put(to, text, content_id, Duration::from_millis(left as u64))
    }
}

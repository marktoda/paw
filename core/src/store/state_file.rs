//! Small files in a store's state directory, next to the repository and
//! never in it: not synced, removed with the store.
//!
//! The app and the AutoFill extension both write some of them. There is no
//! file lock (iOS kills a suspended process that holds one in a shared
//! container, see [`crate::lock`]): each write goes to a temporary file that
//! is renamed over the old one, so a reader sees a whole file or none. Every
//! read is bounded by [`MAX_FILE_BYTES`].

use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use rand::RngCore;
use rand::rngs::OsRng;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::read::MAX_FILE_BYTES;

/// A temporary file this old belongs to a writer that died mid-write.
const STALE_TEMP: Duration = Duration::from_secs(60);

/// The bytes of `path`: `None` if there is no such file, an error if it is
/// larger than [`MAX_FILE_BYTES`].
pub(crate) fn read_bounded(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(std::io::Error::new(ErrorKind::InvalidData, "the file is too large"));
    }
    Ok(Some(bytes))
}

/// Writes `bytes` to `path` (in `dir`), readable by this app only, through
/// a temporary file named `prefix…` that is flushed to disk and renamed over
/// it. Temporary files a dead writer left behind are removed first.
pub(crate) fn write_atomic(dir: &Path, prefix: &str, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    remove_stale_temps(dir, prefix);
    let mut tag = [0u8; 12];
    OsRng.fill_bytes(&mut tag);
    let tag: String = tag.iter().map(|b| format!("{b:02x}")).collect();
    let temp = dir.join(format!("{prefix}{tag}"));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

fn remove_stale_temps(dir: &Path, prefix: &str) {
    let Ok(listing) = std::fs::read_dir(dir) else { return };
    for child in listing.flatten() {
        if !child.file_name().to_str().is_some_and(|n| n.starts_with(prefix)) {
            continue;
        }
        let old = child
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| SystemTime::now().duration_since(at).ok())
            .is_some_and(|age| age >= STALE_TEMP);
        if old {
            let _ = std::fs::remove_file(child.path());
        }
    }
}

pub(crate) fn remove_if_there(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn gone() -> std::io::Error {
    std::io::Error::new(ErrorKind::NotFound, "the store's folder is gone")
}

/// A longer entry path is not kept.
pub(crate) const MAX_PATH_BYTES: usize = 512;
const VERSION: u32 = 1;

/// One item of a [`ListFile`]: about one entry, by its path.
pub(crate) trait ListItem: Serialize + DeserializeOwned {
    fn entry(&self) -> &str;

    /// The item as read from the file, made valid; `None` leaves it out.
    fn clean(self) -> Option<Self> {
        Some(self)
    }
}

impl ListItem for String {
    fn entry(&self) -> &str {
        self
    }
}

/// What one list file is: its name, the JSON key of its list, how many
/// items it keeps, and the turn its writers take in this process.
pub(crate) struct ListKind {
    pub file: &'static str,
    pub key: &'static str,
    pub temp_prefix: &'static str,
    pub max_items: usize,
    pub writing: Mutex<()>,
}

/// A list of items about entries, at most one per entry, least recent
/// first, kept as `{"version":1,"<key>":[…]}`. A missing, unreadable or
/// malformed file has none and is replaced on the next write; past
/// `max_items` the least recent are dropped.
pub(crate) struct ListFile {
    state_dir: PathBuf,
    kind: &'static ListKind,
}

impl ListFile {
    pub fn new(state_dir: &Path, kind: &'static ListKind) -> Self {
        Self { state_dir: state_dir.to_path_buf(), kind }
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn path(&self) -> PathBuf {
        self.state_dir.join(self.kind.file)
    }

    /// The turn of this process's writers.
    pub fn turn(&self) -> MutexGuard<'static, ()> {
        self.kind.writing.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes `bytes` to `path` the way the list itself is written.
    pub fn write_beside(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        write_atomic(&self.state_dir, self.kind.temp_prefix, path, bytes)
    }

    /// Every item, least recent first.
    pub fn load<T: ListItem>(&self) -> Vec<T> {
        let Ok(Some(bytes)) = read_bounded(&self.path()) else { return Vec::new() };
        let Some(items) = self.decode::<T>(&bytes) else { return Vec::new() };
        let mut seen = HashSet::new();
        let mut items: Vec<T> = items
            .into_iter()
            .filter(|item| !item.entry().is_empty() && item.entry().len() <= MAX_PATH_BYTES)
            .filter(|item| seen.insert(item.entry().to_owned()))
            .filter_map(ListItem::clean)
            .collect();
        self.bound(&mut items);
        items
    }

    fn decode<T: ListItem>(&self, bytes: &[u8]) -> Option<Vec<T>> {
        let mut book: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(bytes).ok()?;
        let version: u32 = serde_json::from_value(book.remove("version")?).ok()?;
        if version != VERSION {
            return None;
        }
        serde_json::from_value(book.remove(self.kind.key)?).ok()
    }

    fn bound<T>(&self, items: &mut Vec<T>) {
        let extra = items.len().saturating_sub(self.kind.max_items);
        items.drain(..extra);
    }

    /// Reads, changes and writes the file. Nothing is written if `change`
    /// says it changed nothing, or while `on` says no (asked again after the
    /// write, which is undone if it says no then). True if written. Refuses
    /// if the state directory is gone: it is never made again here.
    pub fn update<T: ListItem>(
        &self,
        on: impl Fn() -> bool,
        change: impl FnOnce(&mut Vec<T>) -> bool,
    ) -> std::io::Result<bool> {
        let _turn = self.turn();
        if !self.state_dir.is_dir() {
            return Err(gone());
        }
        if !on() {
            return Ok(false);
        }
        let mut items = self.load();
        if !change(&mut items) {
            return Ok(false);
        }
        self.bound(&mut items);
        let list = serde_json::to_string(&items).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
        let key = serde_json::to_string(self.kind.key).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
        let bytes = format!("{{\"version\":{VERSION},{key}:{list}}}");
        self.write_beside(&self.path(), bytes.as_bytes())?;
        if !on() {
            remove_if_there(&self.path())?;
            return Ok(false);
        }
        Ok(true)
    }

    /// Removes the file.
    pub fn clear(&self) -> std::io::Result<()> {
        let _turn = self.turn();
        remove_if_there(&self.path())
    }

    /// Refuses unless the state directory is there.
    pub fn require_dir(&self) -> std::io::Result<()> {
        if self.state_dir.is_dir() { Ok(()) } else { Err(gone()) }
    }
}

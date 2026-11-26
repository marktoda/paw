//! Lists of items about entries, in a store's state directory next to the
//! repository and never in it: AutoFill's site pairings and the entries
//! with a one-time code. Not synced, removed with the store.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{MAX_FILE_BYTES, read_bounded, remove_if_there, write_atomic};

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
/// items it keeps, the marker that switches it on if it has one, and the
/// turn its writers take in this process.
pub(crate) struct ListKind {
    pub file: &'static str,
    pub key: &'static str,
    pub temp_prefix: &'static str,
    pub max_items: usize,
    /// A list with a switch is kept only while this file is beside it.
    pub on_marker: Option<&'static str>,
    pub writing: Mutex<()>,
}

/// A list of items about entries, at most one per entry, least recent
/// first, kept as `{"version":1,"<key>":[…]}`. A missing, unreadable or
/// malformed file has none and is replaced on the next write; past
/// `max_items` the least recent are dropped. A list with a switch that is
/// off has none and keeps none.
pub(crate) struct ListFile {
    state_dir: PathBuf,
    kind: &'static ListKind,
}

impl ListFile {
    pub fn new(state_dir: &Path, kind: &'static ListKind) -> Self {
        Self { state_dir: state_dir.to_path_buf(), kind }
    }

    pub fn path(&self) -> PathBuf {
        self.state_dir.join(self.kind.file)
    }

    /// The turn of this process's writers.
    fn turn(&self) -> MutexGuard<'static, ()> {
        self.kind.writing.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn marker(&self) -> Option<PathBuf> {
        self.kind.on_marker.map(|name| self.state_dir.join(name))
    }

    /// Whether the list is kept: always, unless it has a switch that is off.
    pub fn enabled(&self) -> bool {
        self.marker().is_none_or(|marker| marker.exists())
    }

    /// Switches the list on or off. Either way it starts empty: on clears
    /// what a write racing an earlier off may have left; off removes the
    /// marker first, so no new item is kept, then the file. Refuses to
    /// switch on if the state directory is gone.
    pub fn set_enabled(&self, enabled: bool) -> std::io::Result<()> {
        let Some(marker) = self.marker() else { return Ok(()) };
        let _turn = self.turn();
        if enabled {
            if !self.state_dir.is_dir() {
                return Err(gone());
            }
            remove_if_there(&self.path())?;
            return write_atomic(&marker, self.kind.temp_prefix, b"");
        }
        let unmarked = remove_if_there(&marker);
        let removed = remove_if_there(&self.path());
        unmarked.and(removed)
    }

    /// Every item, least recent first.
    pub fn load<T: ListItem>(&self) -> Vec<T> {
        if !self.enabled() {
            return Vec::new();
        }
        let Ok(Some(bytes)) = read_bounded(&self.path(), MAX_FILE_BYTES) else { return Vec::new() };
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
    /// says it changed nothing, or while the list is off (asked again after
    /// the write, which is undone if another process switched it off
    /// meanwhile). True if written. Refuses if the state directory is gone:
    /// it is never made again here.
    pub fn update<T: ListItem>(&self, change: impl FnOnce(&mut Vec<T>) -> bool) -> std::io::Result<bool> {
        let _turn = self.turn();
        if !self.state_dir.is_dir() {
            return Err(gone());
        }
        if !self.enabled() {
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
        write_atomic(&self.path(), self.kind.temp_prefix, bytes.as_bytes())?;
        if !self.enabled() {
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
}

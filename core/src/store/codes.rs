//! Which entries have a one-time code, as far as this device has seen.
//!
//! The store is encrypted, so paw can only know an entry has a code after
//! it has been opened (or saved) here. AutoFill needs to know without
//! opening anything: a request for a one-time code lists only the site's
//! entries known to have one, and the app tells iOS which of its
//! suggestions can fill a code. So each time an entry is opened or saved,
//! whether it has a code that works is kept here: one yes per entry path,
//! nothing else. Never the secret, never a code.
//!
//! The file is `one-time-codes.json` in the store's state directory, next
//! to `site-pairings.json` and written the same way ([`super::pairings`]):
//! not synced, removed with the store, each write a whole file renamed over
//! the old one. Two processes writing in the same instant can lose one
//! mark, which costs one entry missing from the code list until it is
//! opened again. Bounded to [`MAX_ENTRIES`] paths, the least recently
//! marked dropped first.
//!
//! Off until the person turns it on (Settings › AutoFill, "Suggest
//! one-time codes"): which entries have a code is learned from decrypted
//! text, and this is the one such fact paw keeps outside a sealed copy
//! (design 5.6 rule 6). The switch is the marker [`ON_MARKER`] beside the
//! file, so the app and the extension read the same answer. While off
//! nothing is written or read, and turning it off deletes the file.

use std::collections::HashSet;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::PassStore;
use super::pairings::write_atomic;
use crate::PawError;
use crate::ffi::OpenedEntry;

/// The file, in the state directory.
pub const FILE: &str = "one-time-codes.json";
/// Present while the person has the list on.
pub const ON_MARKER: &str = "one-time-codes-on";
/// The most entries kept.
pub const MAX_ENTRIES: usize = 1000;
/// A longer path is not kept.
const MAX_PATH_BYTES: usize = 512;
/// A larger file is not read.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const VERSION: u32 = 1;
const TEMP_PREFIX: &str = ".one-time-codes.tmp-";

#[derive(Serialize, Deserialize)]
struct Book {
    version: u32,
    /// Least recently marked first.
    entries: Vec<String>,
}

/// Threads of this process write one at a time.
static WRITING: Mutex<()> = Mutex::new(());

/// The one-time-code marks of one store.
#[derive(Debug, Clone)]
pub struct CodeFile {
    state_dir: PathBuf,
}

impl CodeFile {
    pub fn new(state_dir: &Path) -> Self {
        Self { state_dir: state_dir.to_path_buf() }
    }

    pub fn path(&self) -> PathBuf {
        self.state_dir.join(FILE)
    }

    /// Whether the person turned the list on.
    pub fn enabled(&self) -> bool {
        self.state_dir.join(ON_MARKER).exists()
    }

    /// Turns the list on or off. Either way it starts empty: on clears
    /// what a write racing an earlier off may have left; off removes the
    /// marker first, so no new mark is kept, then the file.
    pub fn set_enabled(&self, enabled: bool) -> std::io::Result<()> {
        let _turn = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        let marker = self.state_dir.join(ON_MARKER);
        if enabled {
            if !self.state_dir.is_dir() {
                return Err(std::io::Error::new(ErrorKind::NotFound, "the store's folder is gone"));
            }
            remove_if_there(&self.path())?;
            return write_atomic(&self.state_dir, TEMP_PREFIX, &marker, b"");
        }
        let unmarked = remove_if_there(&marker);
        let removed = remove_if_there(&self.path());
        unmarked.and(removed)
    }

    /// Every marked entry, least recently marked first. None while the
    /// list is off; a missing, unreadable or malformed file has none.
    pub fn load(&self) -> Vec<String> {
        if !self.enabled() {
            return Vec::new();
        }
        let Ok(file) = std::fs::File::open(self.path()) else { return Vec::new() };
        let mut bytes = Vec::new();
        if file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > MAX_FILE_BYTES {
            return Vec::new();
        }
        let Ok(book) = serde_json::from_slice::<Book>(&bytes) else { return Vec::new() };
        if book.version != VERSION {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        let mut entries: Vec<String> = book
            .entries
            .into_iter()
            .filter(|e| !e.is_empty() && e.len() <= MAX_PATH_BYTES)
            .filter(|e| seen.insert(e.clone()))
            .collect();
        let extra = entries.len().saturating_sub(MAX_ENTRIES);
        entries.drain(..extra);
        entries
    }

    /// Reads, changes and writes the file; nothing is written if `change`
    /// says it changed nothing, or while the list is off. Refuses if the
    /// state directory is gone.
    fn update(&self, change: impl FnOnce(&mut Vec<String>) -> bool) -> std::io::Result<bool> {
        let _turn = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        if !self.state_dir.is_dir() {
            return Err(std::io::Error::new(ErrorKind::NotFound, "the store's folder is gone"));
        }
        if !self.enabled() {
            return Ok(false);
        }
        let mut entries = self.load();
        if !change(&mut entries) {
            return Ok(false);
        }
        let extra = entries.len().saturating_sub(MAX_ENTRIES);
        entries.drain(..extra);
        let bytes = serde_json::to_vec(&Book { version: VERSION, entries })
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
        write_atomic(&self.state_dir, TEMP_PREFIX, &self.path(), &bytes)?;
        // The other process turned the list off while this one wrote.
        if !self.enabled() {
            remove_if_there(&self.path())?;
            return Ok(false);
        }
        Ok(true)
    }

    /// Keeps whether `entry` has a code. True if that changed the file. A
    /// mark already kept is not moved: an entry opened often is no reason
    /// to write each time.
    pub fn mark(&self, entry: &str, has_code: bool) -> std::io::Result<bool> {
        if entry.is_empty() || entry.len() > MAX_PATH_BYTES {
            return Ok(false);
        }
        self.update(|entries| {
            let known = entries.iter().any(|e| e == entry);
            match (known, has_code) {
                (false, true) => entries.push(entry.to_owned()),
                (true, false) => entries.retain(|e| e != entry),
                _ => return false,
            }
            true
        })
    }

    /// The entry moved from `from` to `to`, or with `None` went: its mark
    /// follows it.
    pub fn moved(&self, from: &str, to: Option<&str>) -> std::io::Result<()> {
        if Some(from) == to {
            return Ok(());
        }
        self.update(|entries| {
            let Some(at) = entries.iter().position(|e| e == from) else { return false };
            entries.remove(at);
            if let Some(to) = to.filter(|to| !to.is_empty() && to.len() <= MAX_PATH_BYTES) {
                entries.retain(|e| e != to);
                entries.insert(at.min(entries.len()), to.to_owned());
            }
            true
        })
        .map(|_| ())
    }

    /// Forgets every mark.
    pub fn clear(&self) -> std::io::Result<()> {
        let _turn = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        remove_if_there(&self.path())
    }
}

fn remove_if_there(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn failed(e: std::io::Error) -> PawError {
    PawError::Store(format!("could not keep which entries have codes: {e}"))
}

/// Which entries of one store have a one-time code, for the app and the
/// AutoFill extension.
#[derive(uniffi::Object)]
pub struct OneTimeCodeEntries {
    file: CodeFile,
}

#[uniffi::export]
impl OneTimeCodeEntries {
    /// The marks of the store whose state directory is `state_dir`. Reads
    /// nothing until asked.
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Arc<Self> {
        Arc::new(Self { file: CodeFile::new(Path::new(&state_dir)) })
    }

    /// `opened`, the entry at `entry`, was just opened: keeps whether it
    /// has a one-time code that works. True if that changed what is kept
    /// (the suggestions above the keyboard are then out of date). False,
    /// and nothing kept, once `opened` is closed or while the list is off.
    pub fn note_opened(&self, entry: String, opened: Arc<OpenedEntry>) -> Result<bool, PawError> {
        let Some(has_code) = opened.makes_codes() else { return Ok(false) };
        self.file.mark(&entry, has_code).map_err(failed)
    }

    /// `note_opened` with the answer already read from the entry
    /// (`OpenedEntry.makes_one_time_codes`), for after the entry is wiped.
    pub fn note(&self, entry: String, has_code: bool) -> Result<bool, PawError> {
        self.file.mark(&entry, has_code).map_err(failed)
    }

    /// Whether the person turned the list on. Off by default.
    pub fn enabled(&self) -> bool {
        self.file.enabled()
    }

    /// Settings › AutoFill › Suggest one-time codes. Off deletes the list;
    /// on starts an empty one.
    pub fn set_enabled(&self, enabled: bool) -> Result<(), PawError> {
        self.file.set_enabled(enabled).map_err(failed)
    }

    /// The entries of `store` known to have a one-time code, sorted. None
    /// while the list is off.
    pub fn entries(&self, store: Arc<PassStore>) -> Vec<String> {
        let existing: HashSet<&str> = store.entries().iter().map(|e| e.path.as_str()).collect();
        let mut out: Vec<String> = self.file.load().into_iter().filter(|e| existing.contains(e.as_str())).collect();
        out.sort();
        out
    }

    /// Forgets every mark.
    pub fn clear(&self) -> Result<(), PawError> {
        self.file.clear().map_err(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITH_CODE: &[u8] = b"pw\notpauth://totp/GitHub:you?secret=JBSWY3DPEHPK3PXP&issuer=GitHub\n";
    const BROKEN_CODE: &[u8] = b"pw\notpauth://totp/GitHub:you?secret=!!!\n";

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn opened(text: &[u8]) -> Arc<OpenedEntry> {
        OpenedEntry::from_plaintext(text.to_vec()).unwrap()
    }

    /// The marks in `dir`, turned on.
    fn codes(dir: &tempfile::TempDir) -> Arc<OneTimeCodeEntries> {
        let codes = OneTimeCodeEntries::open(dir.path().to_str().unwrap().into());
        codes.set_enabled(true).unwrap();
        codes
    }

    fn file_on(dir: &Path) -> CodeFile {
        let file = CodeFile::new(dir);
        file.set_enabled(true).unwrap();
        file
    }

    #[test]
    fn off_by_default_keeps_and_lists_nothing() {
        let d = dir();
        let codes = OneTimeCodeEntries::open(d.path().to_str().unwrap().into());
        assert!(!codes.enabled());
        assert!(!codes.note_opened("web/github.com".into(), opened(WITH_CODE)).unwrap());
        assert!(!codes.note("web/github.com".into(), true).unwrap());
        assert!(!d.path().join(FILE).exists(), "nothing is written while off");
        // A file left by a write that raced an off is not read.
        std::fs::write(d.path().join(FILE), br#"{"version":1,"entries":["a"]}"#).unwrap();
        assert!(codes.file.load().is_empty());
    }

    #[test]
    fn turning_off_deletes_the_list_and_on_starts_empty() {
        let d = dir();
        let codes = codes(&d);
        assert!(codes.enabled());
        assert!(codes.note("a".into(), true).unwrap());
        assert_eq!(codes.file.load(), ["a"]);
        codes.set_enabled(false).unwrap();
        assert!(!codes.enabled());
        assert!(!d.path().join(FILE).exists());
        assert!(!d.path().join(ON_MARKER).exists());
        std::fs::write(d.path().join(FILE), br#"{"version":1,"entries":["stale"]}"#).unwrap();
        codes.set_enabled(true).unwrap();
        assert!(codes.file.load().is_empty(), "on starts empty");
        codes.set_enabled(false).unwrap();
        codes.set_enabled(false).unwrap();
    }

    #[test]
    fn note_keeps_the_answer_given() {
        let d = dir();
        let codes = codes(&d);
        assert!(opened(WITH_CODE).makes_one_time_codes());
        assert!(!opened(BROKEN_CODE).makes_one_time_codes());
        let closed = opened(WITH_CODE);
        closed.close();
        assert!(!closed.makes_one_time_codes());
        assert!(codes.note("a".into(), true).unwrap());
        assert!(!codes.note("a".into(), true).unwrap());
        assert!(codes.note("a".into(), false).unwrap());
        assert!(codes.file.load().is_empty());
    }

    #[test]
    fn opening_marks_an_entry_with_a_working_code_and_nothing_else() {
        let d = dir();
        let codes = codes(&d);
        assert!(codes.note_opened("web/github.com".into(), opened(WITH_CODE)).unwrap());
        assert!(!codes.note_opened("web/github.com".into(), opened(WITH_CODE)).unwrap(), "no change, no write");
        assert!(!codes.note_opened("web/gitlab.com".into(), opened(b"pw\nlogin: you\n")).unwrap());
        assert!(!codes.note_opened("web/broken.com".into(), opened(BROKEN_CODE)).unwrap());
        assert_eq!(codes.file.load(), ["web/github.com"]);
        let text = std::fs::read_to_string(d.path().join(FILE)).unwrap();
        assert!(!text.contains("JBSW") && !text.contains("otpauth") && !text.contains("pw"), "{text}");
    }

    #[test]
    fn a_code_removed_since_unmarks_it() {
        let d = dir();
        let codes = codes(&d);
        codes.note_opened("a".into(), opened(WITH_CODE)).unwrap();
        assert!(codes.note_opened("a".into(), opened(b"pw\n")).unwrap());
        assert!(codes.file.load().is_empty());
    }

    #[test]
    fn a_closed_entry_marks_nothing() {
        let d = dir();
        let codes = codes(&d);
        let entry = opened(WITH_CODE);
        entry.close();
        assert!(!codes.note_opened("a".into(), entry).unwrap());
        assert!(!d.path().join(FILE).exists());
    }

    #[test]
    fn marks_follow_moves_and_go_with_deletes() {
        let d = dir();
        let file = file_on(d.path());
        file.mark("a", true).unwrap();
        file.mark("b", true).unwrap();
        file.moved("a", Some("c")).unwrap();
        assert_eq!(file.load(), ["c", "b"]);
        file.moved("b", None).unwrap();
        assert_eq!(file.load(), ["c"]);
        file.moved("unknown", Some("d")).unwrap();
        assert_eq!(file.load(), ["c"]);
        file.clear().unwrap();
        assert!(file.load().is_empty());
    }

    #[test]
    fn nothing_is_kept_once_the_store_is_gone() {
        let d = dir();
        let state = d.path().join("state");
        let file = CodeFile::new(&state);
        assert!(file.mark("a", true).is_err());
        assert!(!state.exists(), "the state folder is never made again here");
    }

    #[test]
    fn size_is_bounded_and_a_bad_file_is_ignored() {
        let d = dir();
        let file = file_on(d.path());
        let many: Vec<String> = (0..MAX_ENTRIES + 5).map(|i| format!("e{i}")).collect();
        let bytes = serde_json::to_vec(&Book { version: VERSION, entries: many }).unwrap();
        std::fs::write(file.path(), bytes).unwrap();
        let loaded = file.load();
        assert_eq!(loaded.len(), MAX_ENTRIES);
        assert_eq!(loaded[0], "e5", "the least recently marked go first");
        std::fs::write(file.path(), b"{not json").unwrap();
        assert!(file.load().is_empty());
        assert!(file.mark("x", true).unwrap());
        assert_eq!(file.load(), ["x"]);
    }

    #[test]
    fn entries_lists_only_those_still_in_the_store() {
        let d = dir();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(repo.join("web")).unwrap();
        std::fs::write(repo.join(".gpg-id"), "0000\n").unwrap();
        std::fs::write(repo.join("web/github.com.gpg"), b"x").unwrap();
        let store = PassStore::open(repo.to_str().unwrap().into()).unwrap();
        let file = file_on(d.path());
        file.mark("web/gone.example", true).unwrap();
        file.mark("web/github.com", true).unwrap();
        let codes = OneTimeCodeEntries::open(d.path().to_str().unwrap().into());
        assert_eq!(codes.entries(store), ["web/github.com"]);
    }
}

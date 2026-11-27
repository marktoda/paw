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
//! Two processes writing in the same instant can lose one
//! mark, which costs one entry missing from the code list until it is
//! opened again.
//!
//! Off until the person turns it on (Settings › AutoFill, "Suggest
//! one-time codes"): which entries have a code is learned from decrypted
//! text, and this is the one such fact paw keeps outside a sealed copy
//! (design 5.6 rule 6). The switch is the marker [`crate::files::layout::ONE_TIME_CODES_ON`] beside the
//! file, so the app and the extension read the same answer. While off
//! nothing is written or read, and turning it off deletes the file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::PassStore;
use crate::files::list::{ListFile, ListKind, MAX_PATH_BYTES};
use crate::PawError;

pub const MAX_ENTRIES: usize = 1000;

static LIST: ListKind = ListKind {
    file: crate::files::layout::ONE_TIME_CODES,
    key: "entries",
    temp_prefix: ".one-time-codes.tmp-",
    max_items: MAX_ENTRIES,
    on_marker: Some(crate::files::layout::ONE_TIME_CODES_ON),
    writing: Mutex::new(()),
};

pub struct CodeFile {
    list: ListFile,
}

impl CodeFile {
    pub fn new(state_dir: &Path) -> Self {
        Self { list: ListFile::new(state_dir, &LIST) }
    }

    pub fn path(&self) -> PathBuf {
        self.list.path()
    }

    pub fn enabled(&self) -> bool {
        self.list.enabled()
    }

    /// Either way the list starts empty.
    pub fn set_enabled(&self, enabled: bool) -> std::io::Result<()> {
        self.list.set_enabled(enabled)
    }

    /// Every marked entry, least recently marked first. None while the
    /// list is off; a missing, unreadable or malformed file has none.
    pub fn load(&self) -> Vec<String> {
        self.list.load()
    }

    /// True if the file changed. A mark already kept is not moved: an
    /// entry opened often is no reason to write each time.
    pub fn mark(&self, entry: &str, has_code: bool) -> std::io::Result<bool> {
        if entry.is_empty() || entry.len() > MAX_PATH_BYTES {
            return Ok(false);
        }
        self.list.update(|entries: &mut Vec<String>| {
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
        self.list.update(|entries: &mut Vec<String>| {
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

    pub fn clear(&self) -> std::io::Result<()> {
        self.list.clear()
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
    /// Reads nothing until asked.
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Arc<Self> {
        Arc::new(Self { file: CodeFile::new(Path::new(&state_dir)) })
    }

    /// The entry at `entry` was opened or saved: keeps whether it has a
    /// one-time code that works (`OpenedEntry.makes_one_time_codes`). True
    /// if that changed what is kept (the suggestions above the keyboard are
    /// then out of date). False, and nothing kept, while the list is off.
    pub fn note(&self, entry: String, has_code: bool) -> Result<bool, PawError> {
        self.file.mark(&entry, has_code).map_err(failed)
    }

    /// Off by default.
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

    pub fn clear(&self) -> Result<(), PawError> {
        self.file.clear().map_err(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::opened::OpenedEntry;

    const WITH_CODE: &[u8] = b"pw\notpauth://totp/GitHub:you?secret=JBSWY3DPEHPK3PXP&issuer=GitHub\n";
    const BROKEN_CODE: &[u8] = b"pw\notpauth://totp/GitHub:you?secret=!!!\n";

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn opened(text: &[u8]) -> Arc<OpenedEntry> {
        OpenedEntry::decrypted(text.to_vec(), vec![0; 32]).unwrap()
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
        assert!(!codes.note("web/github.com".into(), opened(WITH_CODE).makes_one_time_codes()).unwrap());
        assert!(!codes.note("web/github.com".into(), true).unwrap());
        assert!(!d.path().join(crate::files::layout::ONE_TIME_CODES).exists(), "nothing is written while off");
        // A file left by a write that raced an off is not read.
        std::fs::write(d.path().join(crate::files::layout::ONE_TIME_CODES), br#"{"version":1,"entries":["a"]}"#).unwrap();
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
        assert!(!d.path().join(crate::files::layout::ONE_TIME_CODES).exists());
        assert!(!d.path().join(crate::files::layout::ONE_TIME_CODES_ON).exists());
        std::fs::write(d.path().join(crate::files::layout::ONE_TIME_CODES), br#"{"version":1,"entries":["stale"]}"#).unwrap();
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
        assert!(codes.note("web/github.com".into(), opened(WITH_CODE).makes_one_time_codes()).unwrap());
        assert!(!codes.note("web/github.com".into(), opened(WITH_CODE).makes_one_time_codes()).unwrap(), "no change, no write");
        assert!(!codes.note("web/gitlab.com".into(), opened(b"pw\nlogin: you\n").makes_one_time_codes()).unwrap());
        assert!(!codes.note("web/broken.com".into(), opened(BROKEN_CODE).makes_one_time_codes()).unwrap());
        assert_eq!(codes.file.load(), ["web/github.com"]);
        let text = std::fs::read_to_string(d.path().join(crate::files::layout::ONE_TIME_CODES)).unwrap();
        assert!(!text.contains("JBSW") && !text.contains("otpauth") && !text.contains("pw"), "{text}");
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
        let bytes = serde_json::to_vec(&serde_json::json!({ "version": 1, "entries": many })).unwrap();
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
        let store = PassStore::open(d.path().to_str().unwrap().into()).unwrap();
        let file = file_on(d.path());
        file.mark("web/gone.example", true).unwrap();
        file.mark("web/github.com", true).unwrap();
        let codes = OneTimeCodeEntries::open(d.path().to_str().unwrap().into());
        assert_eq!(codes.entries(store), ["web/github.com"]);
    }
}

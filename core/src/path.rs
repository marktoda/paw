//! Entry paths: the names of entries and folders inside the store, and the
//! rules for them.
//!
//! An entry is `folder/name` (`/` separated, no folder at the top), kept in
//! the file `folder/name.gpg`. The recipients of a folder's entries are in
//! the nearest `.gpg-id` at or above it.
//!
//! Names come from the user or from the repository, and the repository is
//! not trusted: it was cloned from a host that may be hostile. Reading
//! ([`crate::store`]) and writing ([`crate::sync`]) share [`unsafe_path`],
//! so a name refused by one is refused by the other.

use unicode_normalization::UnicodeNormalization;

use crate::PawError;

/// The file that lists a folder's recipients.
pub(crate) const GPG_ID: &str = ".gpg-id";
const ENTRY_EXTENSION: &str = ".gpg";

/// `(folder, name)` of a path; the folder is empty at the top.
pub(crate) fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

pub(crate) fn folder_of(path: &str) -> &str {
    split(path).0
}

pub(crate) fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() { name.to_owned() } else { format!("{folder}/{name}") }
}

pub(crate) fn entry_file(entry: &str) -> String {
    format!("{entry}{ENTRY_EXTENSION}")
}

/// The entry kept in `file`: `file` without `.gpg`. `None` if it is not an
/// entry's file; `.gpg` alone is none.
pub(crate) fn entry_of_file(file: &str) -> Option<&str> {
    file.strip_suffix(ENTRY_EXTENSION).filter(|stem| !stem.is_empty())
}

/// The entry the store's listing shows for `file`, a path in the store:
/// none for a hidden file, one in a hidden folder, a path paw refuses, or a
/// file that isn't an entry's.
pub(crate) fn listed_entry(file: &str) -> Option<&str> {
    if has_hidden_part(file) || unsafe_path(file).is_some() {
        return None;
    }
    entry_of_file(file)
}

pub(crate) fn is_entry_file(file: &str) -> bool {
    file.ends_with(ENTRY_EXTENSION)
}

pub(crate) fn gpg_id_in(folder: &str) -> String {
    join(folder, GPG_ID)
}

pub(crate) fn folder_of_gpg_id(gpg_id: &str) -> &str {
    gpg_id.strip_suffix(GPG_ID).unwrap_or(gpg_id).trim_end_matches('/')
}

/// `folder`, then each folder above it, ending with the top (`""`):
/// `a/b`, `a`, `""`. Where `pass` looks for the `.gpg-id` that governs it.
pub(crate) fn folder_and_parents(folder: &str) -> impl Iterator<Item = &str> {
    let mut next = Some(folder);
    std::iter::from_fn(move || {
        let at = next?;
        next = (!at.is_empty()).then(|| folder_of(at));
        Some(at)
    })
}

/// The `.gpg-id` that governs `folder` among those `has` says exist: in the
/// folder, else the nearest above it.
pub(crate) fn governing(folder: &str, mut has: impl FnMut(&str) -> bool) -> Option<String> {
    folder_and_parents(folder).map(gpg_id_in).find(|file| has(file))
}

/// What makes a path unsafe to write: see [`unsafe_part`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum UnsafePart {
    NulOrBackslash,
    Absolute,
    EmptyComponent,
    DotComponent,
    /// A component that some file system takes for `.git`.
    GitDirectory,
}

impl UnsafePart {
    pub(crate) fn words(self) -> &'static str {
        match self {
            Self::NulOrBackslash => "it contains a NUL or a backslash",
            Self::Absolute => "it is absolute",
            Self::EmptyComponent => "it has an empty component",
            Self::DotComponent => "it has a `.` or `..` component",
            Self::GitDirectory => "it names git's own directory",
        }
    }
}

/// Why `path` cannot name an entry, or `None` if it can. `path` comes from
/// the user or from the repository, and the repository is not trusted.
pub(crate) fn unsafe_part(path: &str) -> Option<UnsafePart> {
    if path.contains(['\0', '\\']) {
        return Some(UnsafePart::NulOrBackslash);
    }
    if path.starts_with('/') {
        return Some(UnsafePart::Absolute);
    }
    for component in path.split('/') {
        if component.is_empty() {
            return Some(UnsafePart::EmptyComponent);
        }
        if component == "." || component == ".." {
            return Some(UnsafePart::DotComponent);
        }
        if is_dot_git(component) {
            return Some(UnsafePart::GitDirectory);
        }
    }
    None
}

/// [`unsafe_part`] in words.
pub(crate) fn unsafe_path(path: &str) -> Option<&'static str> {
    unsafe_part(path).map(UnsafePart::words)
}

pub(crate) fn unsafe_path_err(path: &str, why: &str) -> PawError {
    PawError::UnsafePath(format!("{path}: {why}"))
}

/// Whether `component` is hidden the way `pass` hides things: a leading dot.
pub(crate) fn is_hidden(component: &str) -> bool {
    component.starts_with('.')
}

pub(crate) fn has_hidden_part(path: &str) -> bool {
    path.split('/').any(is_hidden)
}

/// Why the add and edit form refuses a name or a folder name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameRefusal {
    Blank,
    Slash,
    LeadingDot,
    OuterSpace,
    Control,
    Backslash,
    TooLong,
    Unsafe(&'static str),
}

/// Why an entry name (no `/`) is refused for saving, checked in this order.
pub(crate) fn name_refusal(name: &str) -> Option<NameRefusal> {
    if name.trim().is_empty() {
        return Some(NameRefusal::Blank);
    }
    if name.contains('/') {
        return Some(NameRefusal::Slash);
    }
    if is_hidden(name) {
        return Some(NameRefusal::LeadingDot);
    }
    if name.trim() != name {
        return Some(NameRefusal::OuterSpace);
    }
    if name.chars().any(char::is_control) {
        return Some(NameRefusal::Control);
    }
    if name.contains('\\') {
        return Some(NameRefusal::Backslash);
    }
    if entry_file(name).len() > 255 {
        return Some(NameRefusal::TooLong);
    }
    unsafe_path(name).map(NameRefusal::Unsafe)
}

/// Why a folder path (empty for the top) is refused for saving. Control
/// characters and backslashes are one refusal here.
pub(crate) fn folder_refusal(folder: &str) -> Option<NameRefusal> {
    if folder.is_empty() {
        return None;
    }
    for component in folder.split('/') {
        if component.trim().is_empty() {
            return Some(NameRefusal::Blank);
        }
        if is_hidden(component) {
            return Some(NameRefusal::LeadingDot);
        }
        if component.trim() != component {
            return Some(NameRefusal::OuterSpace);
        }
        if component.chars().any(char::is_control) || component.contains('\\') {
            return Some(NameRefusal::Control);
        }
        if component.len() > 255 {
            return Some(NameRefusal::TooLong);
        }
    }
    unsafe_path(folder).map(NameRefusal::Unsafe)
}

/// How a file system that ignores letter case and Unicode normalisation
/// (APFS on iPhone, by default) sees a name. Two names that are the same
/// here are one file on the phone.
pub(crate) fn case_folded(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

/// Code points HFS+ ignores in names, so `.g\u{200C}it` is `.git` there.
/// The same list as git's `is_hfs_dotgit`.
const HFS_IGNORED: [std::ops::RangeInclusive<char>; 4] = [
    '\u{200C}'..='\u{200F}',
    '\u{202A}'..='\u{202E}',
    '\u{206A}'..='\u{206F}',
    '\u{FEFF}'..='\u{FEFF}',
];

pub(crate) fn is_hfs_ignored(c: char) -> bool {
    HFS_IGNORED.iter().any(|range| range.contains(&c))
}

/// The name some file system would actually use for `component`: without
/// characters HFS+ ignores, without an NTFS alternate data stream
/// (`name:stream`) or the trailing dots and spaces NTFS drops, in lower case.
fn file_system_alias(component: &str) -> String {
    let visible: String = component.chars().filter(|c| !is_hfs_ignored(*c)).collect();
    let name = visible.split(':').next().unwrap_or_default();
    name.trim_end_matches(['.', ' ']).to_lowercase()
}

/// Whether some file system would take `component` for `.git`: any letter
/// case, the Windows short name, trailing dots and spaces or an alternate
/// data stream (NTFS), and characters HFS+ ignores.
fn is_dot_git(component: &str) -> bool {
    let name = file_system_alias(component);
    name == ".git" || name == "git~1"
}

/// Whether some file system would take `component` for `.gpg-id` although
/// it is spelled differently (`.GPG-ID`, `.gpg-id.`, `.gpg\u{200C}-id`). paw
/// reads and reports recipients by the exact name, so such a file would
/// change who an entry is encrypted to without paw noticing.
pub(crate) fn is_gpg_id_alias(component: &str) -> bool {
    component != GPG_ID && file_system_alias(component) == GPG_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_and_parents_end_at_the_top() {
        assert_eq!(folder_and_parents("a/b").collect::<Vec<_>>(), ["a/b", "a", ""]);
        assert_eq!(folder_and_parents("").collect::<Vec<_>>(), [""]);
    }

    #[test]
    fn gpg_id_paths_and_their_folders() {
        assert_eq!(gpg_id_in(""), ".gpg-id");
        assert_eq!(gpg_id_in("a/b"), "a/b/.gpg-id");
        assert_eq!(folder_of_gpg_id(".gpg-id"), "");
        assert_eq!(folder_of_gpg_id("work/.gpg-id"), "work");
        assert_eq!(folder_of_gpg_id("a/b/.gpg-id"), "a/b");
        let has = |file: &str| file == "a/.gpg-id" || file == ".gpg-id";
        assert_eq!(governing("a/b", has).as_deref(), Some("a/.gpg-id"));
        assert_eq!(governing("c", has).as_deref(), Some(".gpg-id"));
        assert_eq!(governing("c", |_| false), None);
    }
}

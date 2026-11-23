//! Which names are safe to use inside the store.
//!
//! Names come from the user or from the repository, and the repository is
//! not trusted: it was cloned from a host that may be hostile. Reading
//! ([`crate::store`]) and writing ([`crate::sync`]) share this one check, so
//! a name refused by one is refused by the other.

use crate::PawError;

/// Why `path` cannot name an entry, or `None` if it can. `path` comes from
/// the user or from the repository, and the repository is not trusted.
pub(crate) fn unsafe_path(path: &str) -> Option<&'static str> {
    if path.contains(['\0', '\\']) {
        return Some("it contains a NUL or a backslash");
    }
    if path.starts_with('/') {
        return Some("it is absolute");
    }
    for component in path.split('/') {
        if component.is_empty() {
            return Some("it has an empty component");
        }
        if component == "." || component == ".." {
            return Some("it has a `.` or `..` component");
        }
        if is_dot_git(component) {
            return Some("it names git's own directory");
        }
    }
    None
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
    component != ".gpg-id" && file_system_alias(component) == ".gpg-id"
}

pub(crate) fn unsafe_path_err(path: &str, why: &str) -> PawError {
    PawError::UnsafePath(format!("{path}: {why}"))
}

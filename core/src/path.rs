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

/// Whether some file system would take `component` for `.git`: any letter
/// case, the Windows short name, trailing dots and spaces or an alternate
/// data stream (NTFS), and characters HFS+ ignores.
fn is_dot_git(component: &str) -> bool {
    const HFS_IGNORED: [std::ops::RangeInclusive<char>; 4] = [
        '\u{200C}'..='\u{200F}',
        '\u{202A}'..='\u{202E}',
        '\u{206A}'..='\u{206F}',
        '\u{FEFF}'..='\u{FEFF}',
    ];
    let visible: String = component
        .chars()
        .filter(|c| !HFS_IGNORED.iter().any(|range| range.contains(c)))
        .collect();
    let name = visible.split(':').next().unwrap_or_default();
    let name = name.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    name == ".git" || name == "git~1"
}

pub(crate) fn unsafe_path_err(path: &str, why: &str) -> PawError {
    PawError::UnsafePath(format!("{path}: {why}"))
}

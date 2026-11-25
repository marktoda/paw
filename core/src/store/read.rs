//! Reading files out of the store without ever leaving it.
//!
//! The working copy was written by a git checkout of a repository paw does
//! not trust, so it can contain symbolic links to anywhere on the device. A
//! path is therefore checked by name first ([`crate::path`]) and then walked
//! one component at a time, refusing any link on the way. `symlink_metadata`
//! looks at the link itself, not at what it points to.
//!
//! The walk and the read are separate steps. That is sound because only the
//! app writes to its own container; nothing can swap a folder for a link in
//! between.

use std::io::Read;
use std::path::Path;

use crate::PawError;
use crate::path::{entry_file, has_hidden_part, unsafe_path, unsafe_path_err};

/// No entry or `.gpg-id` comes close to this. A larger file is not read, so
/// a hostile repository cannot make paw load something huge.
pub(crate) const MAX_FILE_BYTES: u64 = 1 << 20;

fn io_err(relative: &str, e: std::io::Error) -> PawError {
    PawError::Store(format!("could not read {relative}: {e}"))
}

/// Reads the regular file `relative` (`/` separated) under `root`. `None` if
/// it, or a folder on the way, does not exist.
pub(crate) fn read_file(root: &Path, relative: &str) -> Result<Option<Vec<u8>>, PawError> {
    if let Some(why) = unsafe_path(relative) {
        return Err(unsafe_path_err(relative, why));
    }
    let mut on_disk = root.to_path_buf();
    let mut components = relative.split('/').peekable();
    while let Some(component) = components.next() {
        on_disk.push(component);
        let is_folder = components.peek().is_some();
        let found = match std::fs::symlink_metadata(&on_disk) {
            Ok(found) => found,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            // A file where a folder was expected.
            Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => return Ok(None),
            Err(e) => return Err(io_err(relative, e)),
        };
        if found.is_symlink() {
            return Err(unsafe_path_err(relative, "it passes through a link"));
        }
        if is_folder && !found.is_dir() {
            return Ok(None);
        }
        if !is_folder {
            if !found.is_file() {
                return Err(unsafe_path_err(relative, "it is not a file"));
            }
            if found.len() > MAX_FILE_BYTES {
                return Err(PawError::Store(format!("{relative} is too large to be an entry")));
            }
        }
    }

    let file = std::fs::File::open(&on_disk).map_err(|e| io_err(relative, e))?;
    let mut bytes = Vec::new();
    // The limit again: the size above was only what the file system said.
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_err(relative, e))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(PawError::Store(format!("{relative} is too large to be an entry")));
    }
    Ok(Some(bytes))
}

/// Reads the encrypted bytes of the entry `entry` (its path without the
/// `.gpg` extension, e.g. `web/github.com`).
///
/// Refused with [`PawError::UnsafePath`]: an absolute path, `..`, anything
/// inside `.git` or another hidden folder, and any path that runs through a
/// symbolic link.
pub fn read_entry(root: &Path, entry: &str) -> Result<Vec<u8>, PawError> {
    // Checked before the extension is added: an empty name must not turn
    // into the hidden file `.gpg`.
    if let Some(why) = unsafe_path(entry) {
        return Err(unsafe_path_err(entry, why));
    }
    if has_hidden_part(entry) {
        return Err(unsafe_path_err(entry, "hidden files are not entries"));
    }
    read_file(root, &entry_file(entry))?
        .ok_or_else(|| PawError::Store(format!("there is no entry called {entry}")))
}

//! The `window/` directory, shared by the app and the AutoFill extension.
//!
//! There is no lock: iOS kills a suspended process that holds a file lock
//! in a shared container. Instead every write goes to a uniquely named
//! temporary file that is then renamed over the copy, so a reader sees a
//! whole old file, a whole new one, or none. A file that vanishes between
//! listing and reading is simply not there.

use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use super::format::{MAX_FILE_BYTES, SUFFIX};
use crate::files::{read_bounded, remove_if_there, remove_temps, too_large};

/// Prefix of a copy being written. A dot keeps it out of [`copy_files`].
const TEMP_PREFIX: &str = ".tmp-";

fn not_found(e: &std::io::Error) -> bool {
    e.kind() == ErrorKind::NotFound
}

/// Every `<stem>.copy` file in `dir`, as `(stem, path)`. An absent
/// directory has none.
pub fn copy_files(dir: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if not_found(&e) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for child in listing {
        let child = child?;
        let name = child.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(SUFFIX)) else { continue };
        if !stem.starts_with('.') {
            out.push((stem.to_owned(), child.path()));
        }
    }
    Ok(out)
}

/// The bytes of the regular file at `path`, or `None` if it is gone. A
/// file that is not a regular file, or too large to be a copy, reads as no
/// bytes: the caller treats it as garbage.
pub fn read(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::symlink_metadata(path) {
        Ok(found) if found.is_file() => {}
        Ok(_) => return Ok(Some(Vec::new())),
        Err(e) if not_found(&e) => return Ok(None),
        Err(e) => return Err(e),
    }
    match read_bounded(path, MAX_FILE_BYTES) {
        Err(e) if too_large(&e) => Ok(Some(Vec::new())),
        read => read,
    }
}

/// Removes `path`; already gone is fine.
pub fn remove(path: &Path) -> std::io::Result<()> {
    remove_if_there(path)
}

/// Writes `bytes` as `dir/<stem>.copy` in one step, creating `dir` if need
/// be ([`crate::files::write_atomic`]).
pub fn write_atomic(dir: &Path, stem: &str, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    crate::files::write_atomic(&dir.join(format!("{stem}{SUFFIX}")), TEMP_PREFIX, bytes)
}

/// Removes temporary files more than a minute old.
pub fn remove_stale_temps(dir: &Path) -> std::io::Result<()> {
    remove_temps(dir, TEMP_PREFIX, false)
}

/// Creates `dir/<name>` (empty) and flushes it and the directory entry to
/// disk, so it is still there after a power loss.
pub fn write_marker(dir: &Path, name: &str) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let file = std::fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(dir.join(name))?;
    file.sync_all()?;
    std::fs::File::open(dir)?.sync_all()
}

/// Removes every copy and temporary file, those of writers still running
/// too (their copies are being ended). Anything else in `dir` is left.
pub fn wipe(dir: &Path) -> std::io::Result<()> {
    for (_, path) in copy_files(dir)? {
        remove(&path)?;
    }
    remove_temps(dir, TEMP_PREFIX, true)
}

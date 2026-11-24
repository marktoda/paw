//! The `window/` directory, shared by the app and the AutoFill extension.
//!
//! There is no lock: iOS kills a suspended process that holds a file lock
//! in a shared container. Instead every write goes to a uniquely named
//! temporary file that is then renamed over the copy, so a reader sees a
//! whole old file, a whole new one, or none. A file that vanishes between
//! listing and reading is simply not there.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rand::RngCore;
use rand::rngs::OsRng;

use super::format::{MAX_FILE_BYTES, SUFFIX};

/// Prefix of a copy being written. A dot keeps it out of [`copy_files`].
const TEMP_PREFIX: &str = ".tmp-";
/// A temporary file this old belongs to a writer that died mid-write.
const STALE_TEMP: Duration = Duration::from_secs(60);

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

/// The bytes of the regular file at `path`, or `None` if it is gone, not a
/// regular file, or too large to be a copy. The caller treats the last two
/// as garbage.
pub fn read(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::symlink_metadata(path) {
        Ok(found) if found.is_file() && found.len() <= MAX_FILE_BYTES => {}
        Ok(_) => return Ok(Some(Vec::new())),
        Err(e) if not_found(&e) => return Ok(None),
        Err(e) => return Err(e),
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if not_found(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// Removes `path`; already gone is fine.
pub fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if !not_found(&e) => Err(e),
        _ => Ok(()),
    }
}

/// Writes `bytes` as `dir/<stem>.copy` in one step: a temporary file, mode
/// 0600, flushed to disk, then renamed over any older copy.
pub fn write_atomic(dir: &Path, stem: &str, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let mut tag = [0u8; 12];
    OsRng.fill_bytes(&mut tag);
    let tag: String = tag.iter().map(|b| format!("{b:02x}")).collect();
    let temp = dir.join(format!("{TEMP_PREFIX}{tag}"));
    let written = (|| {
        let mut file =
            std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, dir.join(format!("{stem}{SUFFIX}")))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Removes temporary files left by a writer that died, and with `all`,
/// those of writers still running too (their copies are being ended).
fn remove_temps(dir: &Path, all: bool) -> std::io::Result<()> {
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if not_found(&e) => return Ok(()),
        Err(e) => return Err(e),
    };
    for child in listing {
        let child = child?;
        if !child.file_name().to_str().is_some_and(|n| n.starts_with(TEMP_PREFIX)) {
            continue;
        }
        let old = child
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| SystemTime::now().duration_since(at).ok())
            .is_some_and(|age| age >= STALE_TEMP);
        if all || old {
            remove(&child.path())?;
        }
    }
    Ok(())
}

/// Removes temporary files more than a minute old.
pub fn remove_stale_temps(dir: &Path) -> std::io::Result<()> {
    remove_temps(dir, false)
}

/// Creates `dir/<name>` (empty) and flushes it and the directory entry to
/// disk, so it is still there after a power loss.
pub fn write_marker(dir: &Path, name: &str) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let file = std::fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(dir.join(name))?;
    file.sync_all()?;
    std::fs::File::open(dir)?.sync_all()
}

/// Removes every copy and temporary file. Anything else in `dir` is left.
pub fn wipe(dir: &Path) -> std::io::Result<()> {
    for (_, path) in copy_files(dir)? {
        remove(&path)?;
    }
    remove_temps(dir, true)
}

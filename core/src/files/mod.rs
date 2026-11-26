//! paw's own files, below everything that uses them.
//!
//! The app and the AutoFill extension both write some of these files.
//! There is no file lock (iOS kills a suspended process that holds one in a
//! shared container, see [`crate::lock`]): each write goes to a uniquely
//! named temporary file, flushed to disk and renamed over the old one, so a
//! reader sees a whole file or none. Every read is bounded.

pub mod layout;
pub(crate) mod list;
pub(crate) mod refs;
pub(crate) mod values;

#[cfg(test)]
mod fixtures;

use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use rand::RngCore;
use rand::rngs::OsRng;

/// No entry, `.gpg-id`, certificate or state file comes close to this. A
/// larger file is not read, so a hostile repository or a damaged file
/// cannot make paw load something huge.
pub(crate) const MAX_FILE_BYTES: u64 = 1 << 20;

/// A temporary file this old belongs to a writer that died mid-write.
const STALE_TEMP: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct TooLarge;

impl std::fmt::Display for TooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the file is too large")
    }
}

impl std::error::Error for TooLarge {}

pub(crate) fn too_large(error: &std::io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<TooLarge>())
}

pub(crate) fn read_bounded(path: &Path, max: u64) -> std::io::Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(std::io::Error::new(ErrorKind::InvalidData, TooLarge));
    }
    Ok(Some(bytes))
}

pub(crate) fn write_atomic(path: &Path, temp_prefix: &str, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    sweep(dir, temp_prefix);
    let mut tag = [0u8; 12];
    OsRng.fill_bytes(&mut tag);
    let tag: String = tag.iter().map(|b| format!("{b:02x}")).collect();
    let temp = dir.join(format!("{temp_prefix}{tag}"));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        // The write failed already; a temporary file left here is swept by
        // the next one.
        let _ = std::fs::remove_file(&temp);
    }
    written
}

fn sweep(dir: &Path, temp_prefix: &str) {
    let _ = remove_temps(dir, temp_prefix, false);
}

/// Removes the temporary files named `temp_prefix…` in `dir` that a writer
/// which died left behind, and with `all` those of writers still running
/// too. An absent `dir` has none.
pub(crate) fn remove_temps(dir: &Path, temp_prefix: &str, all: bool) -> std::io::Result<()> {
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for child in listing {
        let child = child?;
        if !child.file_name().to_str().is_some_and(|n| n.starts_with(temp_prefix)) {
            continue;
        }
        let old = || {
            child
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| SystemTime::now().duration_since(at).ok())
                .is_some_and(|age| age >= STALE_TEMP)
        };
        if all || old() {
            remove_if_there(&child.path())?;
        }
    }
    Ok(())
}

pub(crate) fn remove_if_there(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Identifies one version of an entry: SHA-256 of its `.gpg` file. A pull
/// or an edit that changes the file changes it, which makes an older Face
/// ID copy, or an open edit, stale.
pub fn content_id(ciphertext: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(ciphertext).to_vec()
}

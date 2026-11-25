//! Which keys a folder's entries are meant to be encrypted for.
//!
//! `pass` reads this from `.gpg-id`: the one in the entry's folder, or else
//! the nearest one above it. Each line names a key by fingerprint, key ID or
//! e-mail address. The lines are returned as written; turning them into keys
//! is the job of whoever encrypts.

use std::path::Path;

use super::read::read_file;
use crate::PawError;
use crate::path::{folder_and_parents, gpg_id_in};

/// The key names in a `.gpg-id` file. Like `pass`, this drops everything
/// from a `#` to the end of a line, and lines that are then blank.
pub fn parse_gpg_id(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The hex digits of a line that names a key by fingerprint or key ID, in
/// upper case: without spaces, a `0x` or the `!` that asks GnuPG for exactly
/// this subkey (it names the same key). `None` for any other line.
pub(crate) fn key_digits(line: &str) -> Option<String> {
    let line = line.trim();
    let bare = line.strip_suffix('!').unwrap_or(line);
    let compact: String = bare.chars().filter(|c| !c.is_whitespace()).collect();
    let digits = compact.strip_prefix("0x").or_else(|| compact.strip_prefix("0X")).unwrap_or(&compact);
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_hexdigit())).then(|| digits.to_ascii_uppercase())
}

/// The recipients for entries in `folder` (`/` separated, empty for the top
/// of the store). The folder need not exist yet.
pub fn recipients_for(root: &Path, folder: &str) -> Result<Vec<String>, PawError> {
    let (file, bytes) = governing_gpg_id(root, folder)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| PawError::Store(format!("{file} is not a text file")))?;
    Ok(parse_gpg_id(&text))
}

/// The `.gpg-id` file that decides the recipients for entries in `folder`:
/// its path in the store (e.g. `work/.gpg-id`) and its bytes.
pub fn governing_gpg_id(root: &Path, folder: &str) -> Result<(String, Vec<u8>), PawError> {
    for file in folder_and_parents(folder).map(gpg_id_in) {
        if let Some(bytes) = read_file(root, &file)? {
            return Ok((file, bytes));
        }
    }
    Err(PawError::Store("this store has no .gpg-id file, so it does not say which keys it is for".into()))
}

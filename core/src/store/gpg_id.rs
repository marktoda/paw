//! Which keys a folder's entries are meant to be encrypted for.
//!
//! `pass` reads this from `.gpg-id`: the one in the entry's folder, or else
//! the nearest one above it. Each line names a key by fingerprint, key ID or
//! e-mail address. The lines are returned as written; turning them into keys
//! is the job of whoever encrypts.

use std::path::Path;

use super::read::read_file;
use crate::PawError;

/// The key names in a `.gpg-id` file. Like `pass`, this drops everything
/// from a `#` to the end of a line, and lines that are then blank.
pub fn parse_gpg_id(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The recipients for entries in `folder` (`/` separated, empty for the top
/// of the store). The folder need not exist yet.
pub fn recipients_for(root: &Path, folder: &str) -> Result<Vec<String>, PawError> {
    let mut at = folder;
    loop {
        let file = if at.is_empty() { ".gpg-id".to_owned() } else { format!("{at}/.gpg-id") };
        if let Some(bytes) = read_file(root, &file)? {
            let text = String::from_utf8(bytes)
                .map_err(|_| PawError::Store(format!("{file} is not a text file")))?;
            return Ok(parse_gpg_id(&text));
        }
        if at.is_empty() {
            return Err(PawError::Store(
                "this store has no .gpg-id file, so it does not say which keys it is for".into(),
            ));
        }
        at = at.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
}

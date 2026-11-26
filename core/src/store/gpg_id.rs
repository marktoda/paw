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

/// The text of the `.gpg-id` at `file` (its path in the store), from its
/// bytes. One that is not UTF-8 is refused, the same way wherever paw reads
/// one: saving, the recipients screen, the integrate report and accepting.
pub(crate) fn gpg_id_text<'b>(file: &str, bytes: &'b [u8]) -> Result<&'b str, PawError> {
    std::str::from_utf8(bytes).map_err(|_| PawError::Store(format!("{file} is not a text file")))
}

/// The key names in the `.gpg-id` at `file`, from its bytes; see
/// [`gpg_id_text`] and [`parse_gpg_id`].
pub(crate) fn gpg_id_lines(file: &str, bytes: &[u8]) -> Result<Vec<String>, PawError> {
    Ok(parse_gpg_id(gpg_id_text(file, bytes)?))
}

/// The recipients for entries in `folder` (`/` separated, empty for the top
/// of the store). The folder need not exist yet.
pub fn recipients_for(root: &Path, folder: &str) -> Result<Vec<String>, PawError> {
    let (file, bytes) = governing_gpg_id(root, folder)?;
    gpg_id_lines(&file, &bytes)
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

#[cfg(test)]
mod tests {
    use super::gpg_id_lines;

    #[test]
    fn a_gpg_id_that_is_not_text_names_no_key_anywhere() {
        assert_eq!(gpg_id_lines("work/.gpg-id", b"ABCD # me\n\n").unwrap(), ["ABCD"]);
        let error = gpg_id_lines("work/.gpg-id", b"ABCD\xff\n").unwrap_err();
        assert_eq!(error.to_string(), "work/.gpg-id is not a text file");
    }
}

//! State files of `name=value` lines: the enrolled key, the setup step,
//! the chosen GitHub repository and the git identity. One pair per line;
//! the first line with a name gives its value. A value never holds a line
//! break: each writer refuses or replaces one before it gets here.

use std::path::Path;

use super::{MAX_FILE_BYTES, read_bounded, write_atomic};

/// A `name=value` file as read.
pub(crate) struct Values {
    text: String,
}

impl Values {
    /// `bytes` as values; an error if they are not text.
    pub fn parse(bytes: Vec<u8>) -> std::io::Result<Self> {
        String::from_utf8(bytes)
            .map(|text| Self { text })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// The value of the first line named `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.text.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
    }

    /// The first line.
    pub fn first_line(&self) -> Option<&str> {
        self.text.lines().next()
    }
}

/// The values in `path`, or `None` if there is no such file.
pub(crate) fn read(path: &Path) -> std::io::Result<Option<Values>> {
    read_bounded(path, MAX_FILE_BYTES)?.map(Values::parse).transpose()
}

/// `pairs` as the file holds them: `name=value`, one per line.
pub(crate) fn text<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    pairs.into_iter().map(|(name, value)| format!("{name}={value}\n")).collect()
}

/// Writes `pairs` to `path` in one step.
pub(crate) fn write<'a>(
    path: &Path,
    temp_prefix: &str,
    pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> std::io::Result<()> {
    write_atomic(path, temp_prefix, text(pairs).as_bytes())
}

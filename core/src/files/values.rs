//! State files of `name=value` lines. One pair per line;
//! the first line with a name gives its value. A value never holds a line
//! break: each writer refuses or replaces one before it gets here.

use std::path::Path;

use super::{MAX_FILE_BYTES, read_bounded, write_atomic};

pub(crate) struct Values {
    text: String,
}

impl Values {
    pub fn parse(bytes: Vec<u8>) -> std::io::Result<Self> {
        String::from_utf8(bytes)
            .map(|text| Self { text })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.text.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
    }
}

pub(crate) fn read(path: &Path) -> std::io::Result<Option<Values>> {
    read_bounded(path, MAX_FILE_BYTES)?.map(Values::parse).transpose()
}

pub(crate) fn text<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    pairs.into_iter().map(|(name, value)| format!("{name}={value}\n")).collect()
}

pub(crate) fn write<'a>(path: &Path, pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> std::io::Result<()> {
    write_atomic(path, text(pairs).as_bytes())
}

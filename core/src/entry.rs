//! The text inside an entry, read the way `pass` users write it.
//!
//! The first line is the password. Every later line is a `key: value` field,
//! an `otpauth://` address for one-time codes, or a note. That is a
//! convention, not a format, so [`EntryDoc`] keeps every line exactly as it
//! was: a document that is parsed and written back unchanged is the same
//! bytes, and an edit rewrites only the line it touches. Another tool's
//! entries are never reformatted just because paw opened them.
//!
//! This is plaintext. The document wipes its memory when dropped, has no
//! `Display`, and its `Debug` prints no contents. Nothing here writes to disk
//! or to a log.

use zeroize::{Zeroize, Zeroizing};

use crate::PawError;

/// Keys read as the user name, in order of preference.
const USERNAME_KEYS: [&str; 4] = ["login", "username", "user", "email"];
/// Keys read as the web address, in order of preference.
const URL_KEYS: [&str; 5] = ["url", "link", "website", "web", "site"];
const OTP_SCHEME: &str = "otpauth://";

/// How a line ended in the file. Kept per line, because a file edited on
/// several machines can mix them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    None,
    Lf,
    CrLf,
}

impl Ending {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

struct Line {
    /// The line without its ending.
    text: Zeroizing<String>,
    ending: Ending,
}

/// What a line after the first one is.
enum Kind<'a> {
    Field { key: &'a str, value: &'a str },
    Otp,
    Note,
}

fn is_otp(text: &str) -> bool {
    text.trim_start()
        .get(..OTP_SCHEME.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(OTP_SCHEME))
}

/// Splits `key: value`. A line whose colon starts `://` is an address, not
/// a field, so a pasted link stays a note.
fn split_field(text: &str) -> Option<(&str, &str)> {
    let (key, value) = text.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || value.starts_with("//") {
        return None;
    }
    Some((key, value.trim()))
}

fn kind(text: &str) -> Kind<'_> {
    if is_otp(text) {
        return Kind::Otp;
    }
    match split_field(text) {
        // `otpauth: otpauth://...`, as some tools write it.
        Some((_, value)) if is_otp(value) => Kind::Otp,
        Some((key, value)) => Kind::Field { key, value },
        None => Kind::Note,
    }
}

fn same_key(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// A `key: value` line of an opened entry.
pub struct Field<'a> {
    /// Which line of the document this is; stable until the next edit.
    pub line: usize,
    /// The key as written, without surrounding spaces.
    pub key: &'a str,
    pub value: &'a str,
}

impl Field<'_> {
    /// Whether the value should start hidden on screen: security answers,
    /// PINs and second passwords are secrets too.
    pub fn looks_secret(&self) -> bool {
        let key = self.key.to_lowercase();
        ["pass", "secret", "pin", "answer"].iter().any(|word| key.contains(word))
    }
}

/// The decrypted text of one entry.
pub struct EntryDoc {
    lines: Vec<Line>,
}

impl std::fmt::Debug for EntryDoc {
    /// Says how many lines there are and nothing about what is in them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EntryDoc {{ {} lines, contents withheld }}", self.lines.len())
    }
}

fn no_line_break(what: &str, text: &str) -> Result<(), PawError> {
    if text.contains(['\n', '\r']) {
        return Err(PawError::InvalidField(format!("{what} cannot contain a line break")));
    }
    Ok(())
}

impl EntryDoc {
    /// Reads decrypted bytes. Takes them by value so the only copy left
    /// afterwards is the document's own; the input is wiped either way.
    ///
    /// Bytes that are not UTF-8 are [`PawError::NotText`]: some stores hold
    /// key files and images, and those are never shown or rewritten.
    pub fn parse(plaintext: Vec<u8>) -> Result<Self, PawError> {
        let mut text = String::from_utf8(plaintext).map_err(|e| {
            e.into_bytes().zeroize();
            PawError::NotText
        })?;
        let lines = text
            .split_inclusive('\n')
            .map(|line| {
                let (body, ending) = if let Some(body) = line.strip_suffix("\r\n") {
                    (body, Ending::CrLf)
                } else if let Some(body) = line.strip_suffix('\n') {
                    (body, Ending::Lf)
                } else {
                    (line, Ending::None)
                };
                Line { text: Zeroizing::new(body.to_owned()), ending }
            })
            .collect();
        text.zeroize();
        Ok(Self { lines })
    }

    /// The document as bytes, ready to encrypt. Equal to what was parsed if
    /// nothing was edited.
    pub fn to_bytes(&self) -> Zeroizing<Vec<u8>> {
        let len = self.lines.iter().map(|l| l.text.len() + l.ending.as_str().len()).sum();
        // Sized up front: growing would leave unwiped copies behind.
        let mut out = Zeroizing::new(Vec::with_capacity(len));
        for line in &self.lines {
            out.extend_from_slice(line.text.as_bytes());
            out.extend_from_slice(line.ending.as_str().as_bytes());
        }
        out
    }

    /// The first line. Empty for an entry that holds only fields or notes.
    pub fn password(&self) -> &str {
        self.lines.first().map_or("", |line| line.text.as_str())
    }

    /// Every `key: value` line after the password, in order.
    fn all_fields(&self) -> impl Iterator<Item = Field<'_>> {
        self.lines.iter().enumerate().skip(1).filter_map(|(line, l)| match kind(&l.text) {
            Kind::Field { key, value } => Some(Field { line, key, value }),
            _ => None,
        })
    }

    /// The field with the first of `keys` that the entry has.
    fn first_of(&self, keys: &[&str]) -> Option<Field<'_>> {
        keys.iter()
            .find_map(|wanted| self.all_fields().find(|field| same_key(field.key, wanted)))
    }

    /// The user name: `login`, else `username`, `user` or `email`, in any
    /// letter case.
    pub fn username(&self) -> Option<&str> {
        self.first_of(&USERNAME_KEYS).map(|field| field.value)
    }

    /// The web address: `url`, else `link`, `website`, `web` or `site`.
    pub fn url(&self) -> Option<&str> {
        self.first_of(&URL_KEYS).map(|field| field.value)
    }

    /// Whether the entry carries a one-time-code secret. The address itself
    /// is never handed out: it is the secret.
    pub fn has_otp(&self) -> bool {
        self.lines.iter().skip(1).any(|line| matches!(kind(&line.text), Kind::Otp))
    }

    /// The value of the first field called `key`, in any letter case.
    pub fn field(&self, key: &str) -> Option<&str> {
        self.all_fields().find(|field| same_key(field.key, key)).map(|field| field.value)
    }

    /// The fields that are not shown as the user name or the web address,
    /// in the order they were written.
    pub fn other_fields(&self) -> Vec<Field<'_>> {
        let shown = [self.first_of(&USERNAME_KEYS), self.first_of(&URL_KEYS)]
            .map(|field| field.map(|field| field.line));
        self.all_fields().filter(|field| !shown.contains(&Some(field.line))).collect()
    }

    /// The lines that are neither fields nor one-time-code addresses, joined
    /// with `\n`, without blank lines at either end.
    pub fn notes(&self) -> Zeroizing<String> {
        let notes: Vec<&str> = self
            .lines
            .iter()
            .skip(1)
            .filter(|line| matches!(kind(&line.text), Kind::Note))
            .map(|line| line.text.as_str())
            .collect();
        let blank = |line: &&str| line.trim().is_empty();
        let start = notes.iter().position(|line| !blank(line)).unwrap_or(notes.len());
        let end = notes.iter().rposition(|line| !blank(line)).map_or(start, |last| last + 1);
        Zeroizing::new(notes[start..end].join("\n"))
    }

    /// The line ending a new line gets: whatever the first line uses.
    fn usual_ending(&self) -> Ending {
        match self.lines.first().map(|line| line.ending) {
            Some(Ending::CrLf) => Ending::CrLf,
            _ => Ending::Lf,
        }
    }

    /// Replaces the first line and nothing else.
    pub fn set_password(&mut self, password: &str) -> Result<(), PawError> {
        no_line_break("a password", password)?;
        match self.lines.first_mut() {
            Some(line) => line.text = Zeroizing::new(password.to_owned()),
            None => self.lines.push(Line {
                text: Zeroizing::new(password.to_owned()),
                ending: Ending::Lf,
            }),
        }
        Ok(())
    }

    /// Sets the first field called `key` (in any letter case) to `value`,
    /// keeping the key and the spacing after its colon as written. If there
    /// is no such field, one is added as a new last line.
    pub fn set_field(&mut self, key: &str, value: &str) -> Result<(), PawError> {
        no_line_break("a field", key)?;
        no_line_break("a field", value)?;
        let key = key.trim();
        if key.is_empty() || key.contains(':') {
            return Err(PawError::InvalidField(
                "a field needs a name, and the name cannot contain a colon".into(),
            ));
        }
        if is_otp(value) || value.starts_with("//") {
            return Err(PawError::InvalidField(
                "a field's value cannot start with `//` or be a one-time-code address".into(),
            ));
        }
        let value = value.trim();

        let existing = self.all_fields().find(|field| same_key(field.key, key)).map(|f| f.line);
        if let Some(index) = existing {
            let line = &mut self.lines[index];
            // Everything up to the value: the key, the colon and the spaces
            // or tabs the author put after it.
            let colon = line.text.find(':').map_or(line.text.len(), |at| at + 1);
            let rest = &line.text[colon..];
            let gap = rest.len() - rest.trim_start().len();
            let mut text = Zeroizing::new(String::with_capacity(colon + gap + value.len()));
            text.push_str(&line.text[..colon + gap]);
            text.push_str(value);
            line.text = text;
            return Ok(());
        }

        let ending = self.usual_ending();
        match self.lines.last_mut() {
            // An entry needs its password line before any field.
            None => self.lines.push(Line { text: Zeroizing::new(String::new()), ending }),
            Some(last) if last.ending == Ending::None => last.ending = ending,
            Some(_) => {}
        }
        self.lines.push(Line { text: Zeroizing::new(format!("{key}: {value}")), ending });
        Ok(())
    }
}

/// Editing helpers for the add and edit screens. Each changes only the
/// lines it is about; every other line keeps its bytes and its place.
impl EntryDoc {
    /// An empty document, for a new entry.
    pub fn empty() -> Self {
        Self { lines: Vec::new() }
    }

    /// A second document with the same bytes. Both wipe their memory when
    /// dropped.
    pub fn duplicate(&self) -> Self {
        let mut bytes = self.to_bytes();
        // Valid UTF-8 already, so this cannot fail; an empty document if it
        // somehow did.
        Self::parse(std::mem::take(&mut *bytes)).unwrap_or_else(|_| Self::empty())
    }

    /// Removes line `index`. The line before keeps its ending.
    fn remove_line(&mut self, index: usize) {
        if index < self.lines.len() {
            self.lines.remove(index);
        }
    }

    /// Sets the field found by `find` (the first of a group of keys) to
    /// `value`; adds `key: value` if there is none, and removes the line if
    /// `value` is empty.
    fn set_preferred(&mut self, keys: &[&str], key: &str, value: &str) -> Result<(), PawError> {
        no_line_break("a field", value)?;
        let existing = self.first_of(keys).map(|field| (field.line, field.key.to_owned()));
        match existing {
            Some((line, _)) if value.trim().is_empty() => {
                self.remove_line(line);
                Ok(())
            }
            Some((_, written)) => self.set_field(&written, value),
            None if value.trim().is_empty() => Ok(()),
            None => self.set_field(key, value),
        }
    }

    /// Sets the user name: the line [`Self::username`] reads, else a new
    /// `login:` line. An empty value removes the line.
    pub fn set_username(&mut self, value: &str) -> Result<(), PawError> {
        self.set_preferred(&USERNAME_KEYS, "login", value)
    }

    /// Sets the web address: the line [`Self::url`] reads, else a new
    /// `url:` line. An empty value removes the line.
    pub fn set_url(&mut self, value: &str) -> Result<(), PawError> {
        self.set_preferred(&URL_KEYS, "url", value)
    }

    /// Removes the first field called `key`, in any letter case. Returns
    /// whether there was one.
    pub fn remove_field(&mut self, key: &str) -> bool {
        let found = self.all_fields().find(|field| same_key(field.key, key)).map(|f| f.line);
        if let Some(line) = found {
            self.remove_line(line);
        }
        found.is_some()
    }

    /// Replaces the notes (see [`Self::notes`]) with `notes`. Nothing
    /// changes if they are the same. Otherwise the note lines between the
    /// first and the last non-blank one are removed and the new lines put
    /// where the first one was (at the end if there were none); fields,
    /// one-time-code lines and blank lines outside that block stay.
    ///
    /// A note line that looks like `key: value` reads back as a field: that
    /// is the pass convention, not something paw can change.
    pub fn set_notes(&mut self, notes: &str) -> Result<(), PawError> {
        let wanted: Vec<&str> = notes.split('\n').map(|line| line.trim_end_matches('\r')).collect();
        let blank = |line: &&str| line.trim().is_empty();
        let start = wanted.iter().position(|line| !blank(line)).unwrap_or(wanted.len());
        let end = wanted.iter().rposition(|line| !blank(line)).map_or(start, |last| last + 1);
        let wanted = &wanted[start..end];
        if wanted.iter().any(|line| line.contains('\r')) {
            return Err(PawError::InvalidField("notes cannot contain a lone carriage return".into()));
        }
        let current = self.notes();
        if current.split('\n').filter(|_| !current.is_empty()).eq(wanted.iter().copied()) {
            return Ok(());
        }

        let is_note = |line: &Line| matches!(kind(&line.text), Kind::Note);
        let is_text = |line: &Line| is_note(line) && !line.text.trim().is_empty();
        let first = self.lines.iter().enumerate().skip(1).find(|(_, l)| is_text(l)).map(|(i, _)| i);
        let last = self.lines.iter().enumerate().skip(1).rev().find(|(_, l)| is_text(l)).map(|(i, _)| i);
        let at = match (first, last) {
            (Some(first), Some(last)) => {
                let mut index = last;
                loop {
                    if is_note(&self.lines[index]) {
                        self.lines.remove(index);
                    }
                    if index == first {
                        break;
                    }
                    index -= 1;
                }
                first
            }
            _ => self.lines.len(),
        };

        let ending = self.usual_ending();
        if at == self.lines.len() {
            match self.lines.last_mut() {
                None if !wanted.is_empty() => {
                    self.lines.push(Line { text: Zeroizing::new(String::new()), ending })
                }
                Some(last) if last.ending == Ending::None && !wanted.is_empty() => last.ending = ending,
                _ => {}
            }
        }
        let at = at.min(self.lines.len()).max(usize::from(!self.lines.is_empty()));
        for (offset, line) in wanted.iter().enumerate() {
            self.lines.insert(at + offset, Line { text: Zeroizing::new((*line).to_owned()), ending });
        }
        Ok(())
    }
}

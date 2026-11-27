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

pub mod opened;

use zeroize::{Zeroize, Zeroizing};

use crate::PawError;
use crate::otp::{OtpProblem, Totp};

/// The most decrypted text an entry may hold. Entries are a few lines; a
/// tiny compressed file that expands without end would otherwise exhaust
/// memory, which the AutoFill extension has very little of.
pub(crate) const MAX_PLAINTEXT_BYTES: u64 = 1 << 20;

/// Keys read as the user name, in order of preference.
const USERNAME_KEYS: [&str; 4] = ["login", "username", "user", "email"];
/// Keys read as the web address, in order of preference.
const URL_KEYS: [&str; 5] = ["url", "link", "website", "web", "site"];
/// Keys AutoFill also takes for a web address when it compares an entry's
/// addresses with the site ([`EntryDoc::addresses`]). The entry's own view
/// shows these as ordinary fields.
const MORE_ADDRESS_KEYS: [&str; 4] = ["uri", "loginurl", "siteurl", "websiteurl"];

fn folded_key(key: &str) -> String {
    key.chars().filter(|c| !matches!(c, ' ' | '-' | '_')).flat_map(char::to_lowercase).collect()
}

fn is_address_key(key: &str) -> bool {
    let key = folded_key(key);
    URL_KEYS.iter().chain(&MORE_ADDRESS_KEYS).any(|known| *known == key)
}

pub(crate) fn has_line_break(text: &str) -> bool {
    text.contains(['\n', '\r'])
}

/// Why a field cannot be written as `key: value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldRefusal {
    KeyBlank,
    KeyLineBreak,
    KeyColon,
    ValueLineBreak,
    /// The value would read back as something else: a one-time-code
    /// address, or (starting `//`) a link in a note.
    ValueReadsAsOther,
}

impl FieldRefusal {
    /// The refusal as the form says it; `what` names the value, e.g. "The
    /// value of PIN".
    pub(crate) fn sentence(self, what: &str) -> String {
        match self {
            Self::KeyBlank => "Give the field a name.".into(),
            Self::KeyLineBreak => "A field name can't contain a line break.".into(),
            Self::KeyColon => "A field name can't contain a colon.".into(),
            Self::ValueLineBreak => format!("{what} can't contain a line break."),
            Self::ValueReadsAsOther => format!("{what} can't start with // or be a one-time-code address."),
        }
    }
}

pub(crate) fn key_refusal(key: &str) -> Option<FieldRefusal> {
    if key.trim().is_empty() {
        Some(FieldRefusal::KeyBlank)
    } else if has_line_break(key) {
        Some(FieldRefusal::KeyLineBreak)
    } else if key.contains(':') {
        Some(FieldRefusal::KeyColon)
    } else {
        None
    }
}

pub(crate) fn value_refusal(value: &str) -> Option<FieldRefusal> {
    if has_line_break(value) {
        Some(FieldRefusal::ValueLineBreak)
    } else if is_otp(value) || value.trim().starts_with("//") {
        Some(FieldRefusal::ValueReadsAsOther)
    } else {
        None
    }
}

/// The lines of notes as typed: `\r\n` ends a line as `\n` does. `None` if
/// another `\r` is left, which no note line may hold.
pub(crate) fn note_lines(notes: &str) -> Option<Vec<&str>> {
    let mut lines: Vec<&str> = notes.split('\n').collect();
    let last = lines.len() - 1;
    for line in &mut lines[..last] {
        *line = line.strip_suffix('\r').unwrap_or(line);
    }
    (!lines.iter().any(|line| line.contains('\r'))).then_some(lines)
}

fn trim_blank<'a, 'b>(lines: &'b [&'a str]) -> &'b [&'a str] {
    let blank = |line: &&str| line.trim().is_empty();
    let start = lines.iter().position(|line| !blank(line)).unwrap_or(lines.len());
    let end = lines.iter().rposition(|line| !blank(line)).map_or(start, |last| last + 1);
    &lines[start..end]
}

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
    text: Zeroizing<String>,
    ending: Ending,
}

/// What a line after the first one is.
enum Kind<'a> {
    Field { key: &'a str, value: &'a str },
    /// A one-time-code line; its address starts at byte `start`.
    Otp { start: usize },
    Note,
}

fn is_otp(text: &str) -> bool {
    crate::otp::is_address(text.trim_start())
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
    let offset = |part: &str| part.as_ptr() as usize - text.as_ptr() as usize;
    if is_otp(text) {
        return Kind::Otp { start: offset(text.trim_start()) };
    }
    match split_field(text) {
        // `otpauth: otpauth://...`, as some tools write it.
        Some((_, value)) if is_otp(value) => Kind::Otp { start: offset(value.trim_start()) },
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EntryDoc {{ {} lines, contents withheld }}", self.lines.len())
    }
}

fn no_line_break(what: &str, text: &str) -> Result<(), PawError> {
    if has_line_break(text) {
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

    fn all_fields(&self) -> impl Iterator<Item = Field<'_>> {
        self.lines.iter().enumerate().skip(1).filter_map(|(line, l)| match kind(&l.text) {
            Kind::Field { key, value } => Some(Field { line, key, value }),
            _ => None,
        })
    }

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

    /// The user name to fill: the first user-name field ([`Self::username`]
    /// order) that is not empty.
    pub fn filled_username(&self) -> Option<&str> {
        USERNAME_KEYS.iter().find_map(|wanted| {
            self.all_fields().filter(|field| same_key(field.key, wanted)).map(|field| field.value).find(|v| !v.is_empty())
        })
    }

    /// Every value AutoFill reads as a web address: [`Self::url`], then each
    /// other field whose key names an address, with letter case, spaces,
    /// `-` and `_` ignored (`URL`, `Login URL`, `uri`, `website_url`…).
    pub fn addresses(&self) -> Vec<&str> {
        let others = self.other_fields().into_iter().filter(|field| is_address_key(field.key));
        self.url().into_iter().chain(others.map(|field| field.value)).collect()
    }

    /// Whether the entry carries a one-time-code secret. The address itself
    /// is never handed out: it is the secret.
    pub fn has_otp(&self) -> bool {
        self.lines.iter().skip(1).any(|line| matches!(kind(&line.text), Kind::Otp { .. }))
    }

    fn otp_address(&self) -> Option<(usize, usize)> {
        self.lines.iter().enumerate().skip(1).find_map(|(index, line)| {
            match kind(&line.text) {
                Kind::Otp { start } => Some((index, start)),
                _ => None,
            }
        })
    }

    /// The code generator from the first one-time-code line: `None` if
    /// there is no such line, the reason if it cannot make codes.
    pub fn one_time_code(&self) -> Option<Result<Totp, OtpProblem>> {
        let (index, start) = self.otp_address()?;
        Some(Totp::from_address(&self.lines[index].text[start..]))
    }

    pub fn makes_codes(&self) -> bool {
        matches!(self.one_time_code(), Some(Ok(_)))
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
        Zeroizing::new(trim_blank(&notes).join("\n"))
    }

    fn usual_ending(&self) -> Ending {
        match self.lines.first().map(|line| line.ending) {
            Some(Ending::CrLf) => Ending::CrLf,
            _ => Ending::Lf,
        }
    }

    /// Makes room for a line after the last: ends the last line with
    /// `ending` if it has none, or, in an empty document, adds the
    /// password line an entry needs before anything else.
    fn end_last_line(&mut self, ending: Ending) {
        match self.lines.last_mut() {
            None => self.lines.push(Line { text: Zeroizing::new(String::new()), ending }),
            Some(last) if last.ending == Ending::None => last.ending = ending,
            Some(_) => {}
        }
    }

    fn push_line(&mut self, text: String) {
        let ending = self.usual_ending();
        self.end_last_line(ending);
        self.lines.push(Line { text: Zeroizing::new(text), ending });
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
        if let Some(refusal) = key_refusal(key).or_else(|| value_refusal(value)) {
            return Err(PawError::InvalidField(refusal.sentence(&format!("The value of {}", key.trim()))));
        }
        let key = key.trim();
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

        self.push_line(format!("{key}: {value}"));
        Ok(())
    }
}

/// Editing helpers for the add and edit screens. Each changes only the
/// lines it is about; every other line keeps its bytes and its place.
impl EntryDoc {
    pub fn empty() -> Self {
        Self { lines: Vec::new() }
    }

    /// A second document with the same bytes. Both wipe their memory when
    /// dropped.
    pub fn duplicate(&self) -> Self {
        let lines = self.lines.iter().map(|line| Line { text: line.text.clone(), ending: line.ending }).collect();
        Self { lines }
    }

    /// The line before keeps its ending.
    fn remove_line(&mut self, index: usize) {
        if index < self.lines.len() {
            self.lines.remove(index);
        }
    }

    /// Sets the first of `keys` the entry has to `value`; adds `key: value`
    /// if there is none, and removes the line if `value` is empty.
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

    /// Puts `address` (an `otpauth://` address) in the first one-time-code
    /// line, keeping whatever came before the address on that line, or adds
    /// it as a new last line if there is none.
    pub fn set_one_time_code(&mut self, address: &str) -> Result<(), PawError> {
        no_line_break("a one-time-code address", address)?;
        let address = address.trim();
        if !is_otp(address) {
            return Err(PawError::InvalidField("a one-time-code address starts with otpauth://".into()));
        }
        if let Some((index, start)) = self.otp_address() {
            let line = &mut self.lines[index];
            let mut text = Zeroizing::new(String::with_capacity(start + address.len()));
            text.push_str(&line.text[..start]);
            text.push_str(address);
            line.text = text;
            return Ok(());
        }
        self.push_line(address.to_owned());
        Ok(())
    }

    /// Removes every one-time-code line. Returns whether there was one.
    pub fn remove_one_time_code(&mut self) -> bool {
        let before = self.lines.len();
        let mut index = 0;
        self.lines.retain(|line| {
            index += 1;
            index == 1 || !matches!(kind(&line.text), Kind::Otp { .. })
        });
        self.lines.len() != before
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
        let wanted = note_lines(notes)
            .ok_or_else(|| PawError::InvalidField("notes cannot contain a lone carriage return".into()))?;
        let wanted = trim_blank(&wanted);
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
        if at == self.lines.len() && !wanted.is_empty() {
            self.end_last_line(ending);
        }
        let at = at.min(self.lines.len()).max(usize::from(!self.lines.is_empty()));
        for (offset, line) in wanted.iter().enumerate() {
            self.lines.insert(at + offset, Line { text: Zeroizing::new((*line).to_owned()), ending });
        }
        Ok(())
    }
}

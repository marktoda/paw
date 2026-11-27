//! An opened entry: the decrypted text and the file it came from.
//!
//! The plaintext stays in the core. Swift gets a description with no secret in it
//! ([`EntryView`]) and asks for one value at a time, when the person taps
//! copy or reveal. A string that has crossed to Swift cannot be wiped, so
//! the fewer and the later, the better.

use std::sync::{Arc, Mutex, MutexGuard};

use zeroize::Zeroizing;

use crate::PawError;
use crate::entry::EntryDoc;

/// One `key: value` line, without its value.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FieldView {
    /// Pass this to [`OpenedEntry::field_value`].
    pub id: u32,
    pub key: String,
    /// The key suggests a secret (a PIN, a security answer): start hidden.
    pub starts_hidden: bool,
}

/// What an opened entry contains, with no secret values.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EntryView {
    pub has_password: bool,
    pub has_username: bool,
    pub has_url: bool,
    /// The entry has a one-time-code line; [`OpenedEntry::otp_code`] makes
    /// its code, or [`OpenedEntry::otp_problem`] says why it can't.
    pub has_otp: bool,
    pub has_notes: bool,
    /// The remaining fields, in the order they were written.
    pub fields: Vec<FieldView>,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct OtpCode {
    /// The digits, e.g. `"123456"`.
    pub code: String,
    /// Seconds until the next code: 1 to `period`.
    pub seconds_left: u32,
    pub period: u32,
}

impl std::fmt::Debug for OtpCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OtpCode {{ code withheld, seconds_left: {}, period: {} }}", self.seconds_left, self.period)
    }
}

/// The decrypted text of one entry. [`Self::close`] wipes it; so does
/// letting go of the object.
#[derive(uniffi::Object)]
pub struct OpenedEntry {
    doc: Mutex<Option<EntryDoc>>,
    /// The content id ([`crate::files::content_id`]) of the file the text
    /// was decrypted from.
    content_id: Vec<u8>,
}

impl std::fmt::Debug for OpenedEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenedEntry {{ contents withheld }}")
    }
}

fn field_views(doc: &EntryDoc) -> Vec<FieldView> {
    doc.other_fields()
        .iter()
        .map(|field| FieldView { id: field.line as u32, key: field.key.to_owned(), starts_hidden: field.looks_secret() })
        .collect()
}

impl OpenedEntry {
    pub fn new(doc: EntryDoc, content_id: Vec<u8>) -> Arc<Self> {
        Arc::new(Self { doc: Mutex::new(Some(doc)), content_id })
    }

    pub fn decrypted(plaintext: impl Into<Zeroizing<Vec<u8>>>, content_id: Vec<u8>) -> Result<Arc<Self>, PawError> {
        let mut plaintext = plaintext.into();
        Ok(Self::new(EntryDoc::parse(std::mem::take(&mut *plaintext))?, content_id))
    }

    fn doc(&self) -> MutexGuard<'_, Option<EntryDoc>> {
        // A panic while the lock was held cannot leave the text half
        // written: nothing here edits it.
        self.doc.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What `read` makes of the text; `None` once closed.
    pub(crate) fn read<T>(&self, read: impl FnOnce(&EntryDoc) -> T) -> Option<T> {
        self.doc().as_ref().map(read)
    }

    /// A copy of the text, for starting an edit. `None` once closed.
    pub(crate) fn duplicate_doc(&self) -> Option<EntryDoc> {
        self.read(EntryDoc::duplicate)
    }

    pub(crate) fn content_id(&self) -> &[u8] {
        &self.content_id
    }
}

#[uniffi::export]
impl OpenedEntry {
    /// Whether the entry has a one-time-code line that makes codes, for
    /// `OneTimeCodeEntries.note`. Nothing is read from disk. `false` once
    /// closed.
    pub fn makes_one_time_codes(&self) -> bool {
        self.read(EntryDoc::makes_codes) == Some(true)
    }

    /// What to draw. Everything is `false` and empty once closed.
    pub fn view(&self) -> EntryView {
        let doc = self.doc();
        let Some(doc) = doc.as_ref() else {
            return EntryView {
                has_password: false,
                has_username: false,
                has_url: false,
                has_otp: false,
                has_notes: false,
                fields: Vec::new(),
            };
        };
        EntryView {
            has_password: !doc.password().is_empty(),
            has_username: doc.username().is_some(),
            has_url: doc.url().is_some(),
            has_otp: doc.has_otp(),
            has_notes: !doc.notes().is_empty(),
            fields: field_views(doc),
        }
    }

    /// The password. `None` once closed.
    pub fn password(&self) -> Option<String> {
        self.doc().as_ref().map(|doc| doc.password().to_owned())
    }

    pub fn username(&self) -> Option<String> {
        self.doc().as_ref()?.username().map(str::to_owned)
    }

    pub fn url(&self) -> Option<String> {
        self.doc().as_ref()?.url().map(str::to_owned)
    }

    /// The value of one of the fields listed by [`Self::view`].
    pub fn field_value(&self, id: u32) -> Option<String> {
        let doc = self.doc();
        let fields = doc.as_ref()?.other_fields();
        let field = fields.iter().find(|field| field.line as u32 == id)?;
        Some(field.value.to_owned())
    }

    /// The one-time code at `now_ms` (milliseconds since 1970, from the
    /// app's clock). `None` once closed, if the entry has no one-time-code
    /// line, or if the line can't make codes ([`Self::otp_problem`]).
    pub fn otp_code(&self, now_ms: i64) -> Option<OtpCode> {
        let doc = self.doc();
        let totp = doc.as_ref()?.one_time_code()?.ok()?;
        let secs = u64::try_from(now_ms).unwrap_or(0) / 1000;
        Some(OtpCode {
            code: totp.code_at(secs).as_str().to_owned(),
            seconds_left: totp.seconds_left(secs),
            period: totp.period(),
        })
    }

    /// Why the entry's one-time-code line can't make codes, in a short
    /// sentence; `None` if it can, if there is none, or once closed.
    pub fn otp_problem(&self) -> Option<String> {
        let doc = self.doc();
        doc.as_ref()?.one_time_code()?.err().map(|why| why.reason().to_owned())
    }

    pub fn notes(&self) -> Option<String> {
        self.doc().as_ref().map(|doc| doc.notes().as_str().to_owned())
    }

    /// Wipes the plaintext now. Every accessor returns `None` afterwards.
    pub fn close(&self) {
        *self.doc() = None;
    }
}

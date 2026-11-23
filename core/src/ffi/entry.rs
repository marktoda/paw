//! An opened entry, as Swift holds it.
//!
//! The plaintext stays here. Swift gets a description with no secret in it
//! ([`EntryView`]) and asks for one value at a time, when the person taps
//! copy or reveal. A string that has crossed to Swift cannot be wiped, so
//! the fewer and the later, the better.

use std::sync::{Arc, Mutex, MutexGuard};

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
    /// The entry holds a one-time-code secret. paw does not show codes yet.
    pub has_otp: bool,
    pub has_notes: bool,
    /// The remaining fields, in the order they were written.
    pub fields: Vec<FieldView>,
}

/// The decrypted text of one entry. [`Self::close`] wipes it; so does
/// letting go of the object.
#[derive(uniffi::Object)]
pub struct OpenedEntry {
    doc: Mutex<Option<EntryDoc>>,
}

impl OpenedEntry {
    fn doc(&self) -> MutexGuard<'_, Option<EntryDoc>> {
        // A panic while the lock was held cannot leave the text half
        // written: nothing here edits it.
        self.doc.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A copy of the text, for starting an edit. `None` once closed.
    pub(crate) fn duplicate_doc(&self) -> Option<EntryDoc> {
        self.doc().as_ref().map(EntryDoc::duplicate)
    }
}

#[uniffi::export]
impl OpenedEntry {
    /// Wraps text that is already decrypted. `open_entry` on the store is
    /// the usual way in; this is for previews and tests.
    #[uniffi::constructor]
    pub fn from_plaintext(plaintext: Vec<u8>) -> Result<Arc<Self>, PawError> {
        let doc = EntryDoc::parse(plaintext)?;
        Ok(Arc::new(Self { doc: Mutex::new(Some(doc)) }))
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
            fields: doc
                .other_fields()
                .iter()
                .map(|field| FieldView {
                    id: field.line as u32,
                    key: field.key.to_owned(),
                    starts_hidden: field.looks_secret(),
                })
                .collect(),
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

    pub fn notes(&self) -> Option<String> {
        self.doc().as_ref().map(|doc| doc.notes().as_str().to_owned())
    }

    /// Wipes the plaintext now. Every accessor returns `None` afterwards.
    pub fn close(&self) {
        *self.doc() = None;
    }
}

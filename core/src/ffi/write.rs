//! The add and edit screens as Swift sees them: an [`EntryDraft`] to fill
//! in, a [`StoreWriter`] that saves and deletes and manages the pinned
//! recipient keys, and the password generator.
//!
//! As with [`OpenedEntry`], the text being edited stays on the Rust side.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::copies::CopyCache;
use crate::entry::opened::OpenedEntry;
use crate::generate::{self, PasswordPolicy};
use crate::store::PassStore;
use crate::write::{self as core, Deleted, DraftProblem, FolderRecipients, Saved, WriteError, WriteStore};

/// The add or edit form's contents. Make one with
/// [`StoreWriter::new_draft`] or [`StoreWriter::edit_draft`].
#[derive(uniffi::Object)]
pub struct EntryDraft {
    inner: Mutex<core::EntryDraft>,
}

impl EntryDraft {
    fn draft(&self) -> MutexGuard<'_, core::EntryDraft> {
        // Each setter replaces one value whole, so a panic elsewhere cannot
        // leave it half written.
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn wrap(draft: core::EntryDraft) -> Arc<Self> {
        Arc::new(Self { inner: Mutex::new(draft) })
    }
}

#[uniffi::export]
impl EntryDraft {
    pub fn set_name(&self, name: String) {
        self.draft().set_name(&name);
    }

    pub fn set_folder(&self, folder: String) {
        self.draft().set_folder(&folder);
    }

    pub fn set_password(&self, password: String) {
        self.draft().set_password(&password);
    }

    /// The Generate button: puts a new password in the form and returns it
    /// to show.
    pub fn generate_password(&self, policy: PasswordPolicy) -> Result<String, WriteError> {
        Ok(self.draft().generate_password(&policy)?.as_str().to_owned())
    }

    pub fn set_username(&self, username: String) {
        self.draft().set_username(&username);
    }

    pub fn set_url(&self, url: String) {
        self.draft().set_url(&url);
    }

    pub fn set_field(&self, key: String, value: String) {
        self.draft().set_field(&key, &value);
    }

    pub fn remove_field(&self, key: String) {
        self.draft().remove_field(&key);
    }

    pub fn set_notes(&self, notes: String) {
        self.draft().set_notes(&notes);
    }

    /// A base32 setup key or an `otpauth://` address for the one-time code,
    /// replacing any the entry has. Blank undoes an earlier set or remove.
    pub fn set_one_time_code(&self, setup: String) {
        self.draft().set_one_time_code(&setup);
    }

    pub fn remove_one_time_code(&self) {
        self.draft().remove_one_time_code();
    }

    /// The "Text" toggle: replaces the whole entry with `text`.
    pub fn set_text(&self, text: String) {
        self.draft().set_text(&text);
    }

    /// What would stop the draft from saving: the values typed and, with
    /// `store`, a name already used among the names it read when it was
    /// opened. No lock and no disk, so the form can ask on every keystroke;
    /// `store` can be out of date, and saving checks again with the store
    /// locked.
    pub fn problems(&self, store: Option<Arc<PassStore>>) -> Vec<DraftProblem> {
        self.draft().problems(store.as_ref().map(|store| store.tree()))
    }
}

/// What would stop the form from setting the field `key` to `value`, if
/// anything. For one row of the form, before it goes into the draft.
#[uniffi::export]
pub fn field_problem(key: String, value: String) -> Option<DraftProblem> {
    core::draft::field_problem(&key, Some(&value))
}

/// Writing to the store set up in a state directory. Every call works
/// without the key and without the network.
#[derive(uniffi::Object)]
pub struct StoreWriter {
    store: WriteStore,
}

#[uniffi::export]
impl StoreWriter {
    /// The store set up in `state_dir`.
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Arc<Self> {
        Arc::new(Self { store: WriteStore::open(&PathBuf::from(state_dir)) })
    }

    /// A blank "New entry" form in `folder`.
    pub fn new_draft(&self, folder: String) -> Arc<EntryDraft> {
        EntryDraft::wrap(core::EntryDraft::new(&folder))
    }

    /// An edit form for `entry`, filled from `opened` (which must be that
    /// entry). Saving is refused if the file changed since it was opened.
    pub fn edit_draft(&self, opened: Arc<OpenedEntry>, entry: String) -> Result<Arc<EntryDraft>, WriteError> {
        let doc = opened.duplicate_doc().ok_or_else(|| WriteError::Failed { message: "the entry was closed".into() })?;
        let content_id = opened.content_id().to_vec();
        Ok(EntryDraft::wrap(core::EntryDraft::edit(&entry, doc, content_id)))
    }

    /// Saves `draft` as one commit (two if an edit also moved it). With
    /// `copies`: an entry that had a live Face ID copy gets it re-sealed with
    /// the new text for the time it had left; a new entry gets a copy for
    /// `new_copy_ttl_secs` (0 keeps none).
    pub fn save(
        &self,
        draft: Arc<EntryDraft>,
        copies: Option<Arc<CopyCache>>,
        new_copy_ttl_secs: u32,
    ) -> Result<Saved, WriteError> {
        let store = self.store.with_copies(copies, Duration::from_secs(new_copy_ttl_secs.into()));
        store.save(&draft.draft())
    }

    /// Deletes `entry` (it stays in the git history) and wipes its copy.
    pub fn delete(&self, entry: String, copies: Option<Arc<CopyCache>>) -> Result<Deleted, WriteError> {
        self.store.with_copies(copies, Duration::ZERO).delete(&entry)
    }

    /// Which keys can open entries in `folder`, and whether paw can save
    /// there (`writable`). For the folder picker and the recipients screen.
    pub fn folder_recipients(&self, folder: String) -> Result<FolderRecipients, WriteError> {
        self.store.folder_recipients(&folder)
    }

    /// "Who can open these passwords"; see [`WriteStore::recipient_scopes`].
    pub fn recipient_scopes(&self) -> Result<Vec<crate::write::RecipientScope>, WriteError> {
        self.store.recipient_scopes()
    }

    /// After "Who can open … changed": accepts the folder's current
    /// `.gpg-id`.
    pub fn accept_recipients(&self, folder: String) -> Result<(), WriteError> {
        self.store.accept_recipients(&folder)
    }
}

/// The Generate button's fixed settings.
#[uniffi::export]
pub fn default_password_policy() -> PasswordPolicy {
    PasswordPolicy::default()
}

/// How strong a password from `policy` is, in bits.
#[uniffi::export]
pub fn password_entropy_bits(policy: PasswordPolicy) -> Result<f64, WriteError> {
    Ok(generate::entropy_bits(&policy)?)
}

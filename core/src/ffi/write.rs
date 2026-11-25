//! The add and edit screens as Swift sees them: an [`EntryDraft`] to fill
//! in, a [`StoreWriter`] that saves, deletes, renames and moves, manages
//! the pinned recipient keys, and the password generator.
//!
//! As with [`super::OpenedEntry`], the text being edited stays on the Rust
//! side; Swift reads one value at a time to fill the form.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use super::{Adapter, CardConnection, FieldView, OpenedEntry};
use crate::copies::{CopyCache, content_id};
use crate::generate::{self, PasswordPolicy};
use crate::store::codes::CodeFile;
use crate::store::PassStore;
use crate::store::pairings::PairingFile;
use crate::store::tree::StoreTree;
use crate::path::join;
use crate::write::cert::certificate_infos;
use crate::write::keyring::{CertificateSummary, certificate_summaries};
use crate::write::{self as core, DraftProblem, FolderRecipients, GitIdentity, WriteError, WriteStore};

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
    pub fn name(&self) -> String {
        self.draft().name().to_owned()
    }

    pub fn folder(&self) -> String {
        self.draft().folder().to_owned()
    }

    /// The entry being edited; `None` for a new one.
    pub fn editing(&self) -> Option<String> {
        self.draft().original_path().map(str::to_owned)
    }

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

    /// Whether the entry as it would be saved has a one-time-code line.
    pub fn has_one_time_code(&self) -> bool {
        self.draft().preview().has_otp()
    }

    /// The "Text" toggle: replaces the whole entry with `text`.
    pub fn set_text(&self, text: String) {
        self.draft().set_text(&text);
    }

    /// Problems with the values typed, without looking at the store; the
    /// store's `validate` also checks the name is free.
    pub fn problems(&self) -> Vec<DraftProblem> {
        let draft = self.draft();
        let mut problems = crate::write::draft::name_problems(draft.name());
        problems.extend(crate::write::draft::folder_problems(draft.folder()));
        problems.extend(draft.value_problems());
        problems
    }

    /// The writer's `validate`, against the names `store` read when it was
    /// opened: no lock and no disk, so the form can ask on every keystroke.
    /// `store` can be out of date; saving checks again with the store locked.
    pub fn problems_in(&self, store: Arc<PassStore>) -> Vec<DraftProblem> {
        self.draft().problems(Some(store.tree()))
    }

    /// The password as it would be saved.
    pub fn password(&self) -> String {
        self.draft().preview().password().to_owned()
    }

    pub fn username(&self) -> Option<String> {
        self.draft().preview().username().map(str::to_owned)
    }

    pub fn url(&self) -> Option<String> {
        self.draft().preview().url().map(str::to_owned)
    }

    pub fn notes(&self) -> String {
        self.draft().preview().notes().as_str().to_owned()
    }

    /// The other fields as they would be saved, without values.
    pub fn fields(&self) -> Vec<FieldView> {
        self.draft()
            .preview()
            .other_fields()
            .iter()
            .map(|field| FieldView { id: field.line as u32, key: field.key.to_owned(), starts_hidden: field.looks_secret() })
            .collect()
    }

    /// The value of one field listed by [`Self::fields`].
    pub fn field_value(&self, id: u32) -> Option<String> {
        let doc = self.draft().preview();
        let fields = doc.other_fields();
        fields.iter().find(|field| field.line as u32 == id).map(|field| field.value.to_owned())
    }

    /// The whole text as it would be saved, for the "Text" toggle.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.draft().preview().to_bytes()).into_owned()
    }
}

/// What a save did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SaveOutcome {
    /// Where the entry is now.
    pub entry: String,
    /// Where it was, if the save moved it.
    pub previous: Option<String>,
    /// The content id of the saved file.
    pub content_id: Vec<u8>,
    /// How many commits were made (0 if nothing changed).
    pub commits: u32,
    /// Until when the entry can be opened by Face ID, if it has a copy.
    pub face_id_until_ms: Option<i64>,
}

/// Writing to the store set up in a state directory. Every call works
/// without the key and without the network.
#[derive(uniffi::Object)]
pub struct StoreWriter {
    store: WriteStore,
}

impl StoreWriter {
    fn tree(&self) -> Result<StoreTree, WriteError> {
        Ok(StoreTree::scan(self.store.repo())?)
    }
}

/// An entry moved or went: its site pairings follow it. A pairing that
/// could not be moved or removed is harmless: it is only ever used for an
/// entry at that path that matches the site by name.
fn pairings_moved(store: &WriteStore, from: &str, to: Option<&str>) {
    let file = PairingFile::new(store.state_dir());
    let _ = match to {
        Some(to) => file.moved(from, to),
        None => file.clear(Some(from)),
    };
    // So does whether it has a one-time code.
    let _ = CodeFile::new(store.state_dir()).moved(from, to);
}

fn forget_copy(copies: Option<&Arc<CopyCache>>, entry: &str) {
    if let Some(copies) = copies {
        // The entry is gone either way; a copy that could not be removed
        // is stale and is wiped by the next read.
        let _ = copies.forget(entry);
    }
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
    /// entry, just opened). Saving is refused if the file changes before.
    pub fn edit_draft(&self, opened: Arc<OpenedEntry>, entry: String) -> Result<Arc<EntryDraft>, WriteError> {
        let doc = opened.duplicate_doc().ok_or_else(|| WriteError::Failed { message: "the entry was closed".into() })?;
        let bytes = {
            let _reading = self.store.lock().shared()?;
            crate::store::read::read_entry(self.store.repo(), &entry)?
        };
        Ok(EntryDraft::wrap(core::EntryDraft::edit(&entry, doc, Some(content_id(&bytes)))))
    }

    /// Everything that would stop `draft` from saving, including a name
    /// that is taken.
    pub fn validate(&self, draft: Arc<EntryDraft>) -> Result<Vec<DraftProblem>, WriteError> {
        let tree = {
            let _reading = self.store.lock().shared()?;
            self.tree()?
        };
        Ok(draft.draft().problems(Some(&tree)))
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
    ) -> Result<SaveOutcome, WriteError> {
        let draft = draft.draft();
        let (saved, doc) = self.store.save(&draft)?;
        if let Some(previous) = &saved.previous {
            pairings_moved(&self.store, previous, Some(&saved.entry));
        }
        // AutoFill learns of a code added or removed here without the
        // entry being opened again. Failing only leaves the mark as it was.
        let _ = CodeFile::new(self.store.state_dir()).mark(&saved.entry, doc.makes_codes());
        let face_id_until_ms = copies.as_ref().and_then(|copies| {
            let text = doc.to_bytes();
            // The entry is saved; a copy that cannot be made only means the
            // next open needs the key.
            match draft.original_path() {
                None => copies
                    .put(&saved.entry, &text, &saved.content_id, Duration::from_secs(new_copy_ttl_secs.into()))
                    .ok()
                    .flatten(),
                Some(original) => copies.reseal(original, &saved.entry, &text, &saved.content_id).ok().flatten(),
            }
        });
        Ok(SaveOutcome {
            entry: saved.entry,
            previous: saved.previous,
            content_id: saved.content_id,
            commits: saved.commits.len() as u32,
            face_id_until_ms,
        })
    }

    /// Deletes `entry` (it stays in the git history) and wipes its copy.
    pub fn delete(&self, entry: String, copies: Option<Arc<CopyCache>>) -> Result<(), WriteError> {
        self.store.delete(&entry)?;
        forget_copy(copies.as_ref(), &entry);
        pairings_moved(&self.store, &entry, None);
        Ok(())
    }

    /// Renames or moves `entry` to `name` in `folder` without opening it.
    /// Refused with `NeedsOpenEntry` if the folder has other recipients:
    /// use [`Self::move_open_entry`]. Its Face ID copy is dropped: moving it
    /// would mean opening it, which needs Face ID. Returns the new path.
    pub fn rename(
        &self,
        entry: String,
        folder: String,
        name: String,
        copies: Option<Arc<CopyCache>>,
    ) -> Result<String, WriteError> {
        let saved = self.store.rename(&entry, &join(folder.trim_matches('/'), &name), None)?;
        if saved.previous.is_some() {
            forget_copy(copies.as_ref(), &entry);
            pairings_moved(&self.store, &entry, Some(&saved.entry));
        }
        Ok(saved.entry)
    }

    /// Moves the open entry `entry` to `name` in `folder`, re-encrypting it
    /// for that folder's recipients if they differ. A live Face ID copy
    /// moves with it.
    pub fn move_open_entry(
        &self,
        entry: String,
        opened: Arc<OpenedEntry>,
        folder: String,
        name: String,
        copies: Option<Arc<CopyCache>>,
    ) -> Result<SaveOutcome, WriteError> {
        let doc = opened.duplicate_doc().ok_or_else(|| WriteError::Failed { message: "the entry was closed".into() })?;
        let saved = self.store.rename(&entry, &join(folder.trim_matches('/'), &name), Some(&doc))?;
        if let Some(previous) = &saved.previous {
            pairings_moved(&self.store, previous, Some(&saved.entry));
        }
        let face_id_until_ms = match (&copies, &saved.previous) {
            (Some(copies), Some(previous)) => copies.reseal(previous, &saved.entry, &doc.to_bytes(), &saved.content_id).ok().flatten(),
            _ => None,
        };
        Ok(SaveOutcome {
            entry: saved.entry,
            previous: saved.previous,
            content_id: saved.content_id,
            commits: saved.commits.len() as u32,
            face_id_until_ms,
        })
    }

    /// Which keys can open entries in `folder`, and whether paw can save
    /// there (`writable`). For the folder picker and the recipients screen.
    pub fn folder_recipients(&self, folder: String) -> Result<FolderRecipients, WriteError> {
        let _reading = self.store.lock().shared()?;
        self.store.folder_recipients(&folder)
    }

    /// "Who can open these passwords": every `.gpg-id` in the store, top
    /// first, with its keys and the entries under it encrypted for a key
    /// it does not name. Reads headers only; decrypts nothing.
    pub fn recipient_scopes(&self) -> Result<Vec<crate::write::RecipientScope>, WriteError> {
        let _reading = self.store.lock().shared()?;
        let keyring = self.store.keyring()?;
        crate::write::scopes::recipient_scopes(&keyring, self.store.repo(), core::now_secs())
    }

    /// Whether paw can save in `folder` now.
    pub fn can_save_in(&self, folder: String) -> bool {
        let Ok(_reading) = self.store.lock().shared() else {
            return false;
        };
        self.store.folder_recipients(&folder).is_ok_and(|r| r.writable())
    }

    /// Setup, once the store is cloned and the person has seen who can open
    /// it: records every `.gpg-id` as accepted. Until then nothing can be
    /// saved; after it, any change to who can open a folder has to be
    /// accepted for that folder.
    pub fn accept_all_recipients_at_setup(&self) -> Result<(), WriteError> {
        // Each keyring change reads, changes and writes the whole file: one
        // critical section, or two changes at once would lose one.
        let _held = self.store.exclusive()?;
        self.store.keyring()?.accept_all_recipients_at_setup(self.store.repo())
    }

    /// After "Who can open … changed": accepts the folder's current
    /// `.gpg-id`.
    pub fn accept_recipients(&self, folder: String) -> Result<(), WriteError> {
        let _held = self.store.exclusive()?;
        self.store.keyring()?.accept_recipients(self.store.repo(), &folder)
    }

    /// Setup: pins the plugged-in key's decryption key. Needs no PIN.
    /// Returns its fingerprint.
    pub fn pin_card_key(&self, connection: Arc<dyn CardConnection>) -> Result<String, WriteError> {
        // The card is read first, with no lock held.
        let key = crate::card_decryption_key(Arc::new(Adapter(connection)))?;
        let _held = self.store.exclusive()?;
        self.store.keyring()?.pin_card_key(&key)
    }

    /// Setup, after the test decrypt succeeded: binds the one top-level
    /// `.gpg-id` line no pinned key satisfies to the card's key. Returns the
    /// lines bound.
    pub fn bind_card_after_test_decrypt(&self, card_fingerprint: String) -> Result<Vec<String>, WriteError> {
        let _held = self.store.exclusive()?;
        self.store.keyring()?.bind_card_after_test_decrypt(self.store.repo(), &card_fingerprint, core::now_secs())
    }

    /// Records that `line` of a `.gpg-id` means the pinned key `fingerprint`.
    pub fn bind_recipient(&self, line: String, fingerprint: String) -> Result<(), WriteError> {
        let _held = self.store.exclusive()?;
        self.store.keyring()?.bind(&line, &fingerprint)
    }

    /// Reads a public certificate (armored or binary) without keeping it.
    pub fn inspect_certificate(&self, bytes: Vec<u8>) -> Result<Vec<CertificateSummary>, WriteError> {
        let infos = certificate_infos(&bytes, core::now_secs())?;
        let _reading = self.store.lock().shared()?;
        certificate_summaries(self.store.repo(), &infos)
    }

    /// Pins the encryption key of each certificate in `bytes`.
    pub fn import_certificate(&self, bytes: Vec<u8>) -> Result<Vec<CertificateSummary>, WriteError> {
        let infos = certificate_infos(&bytes, core::now_secs())?;
        let _held = self.store.exclusive()?;
        let mut keyring = self.store.keyring()?;
        for info in &infos {
            keyring.pin_certificate(info)?;
        }
        certificate_summaries(self.store.repo(), &infos)
    }

    /// Forgets a pinned key.
    pub fn forget_key(&self, fingerprint: String) -> Result<(), WriteError> {
        let _held = self.store.exclusive()?;
        self.store.keyring()?.forget(&fingerprint)
    }

    /// The name and address on paw's commits.
    pub fn git_identity(&self) -> GitIdentity {
        core::git_identity(self.store.state_dir())
    }

    /// Sets the name and address on later commits; `None` goes back to
    /// "paw".
    pub fn set_git_identity(&self, identity: Option<GitIdentity>) -> Result<(), WriteError> {
        let _held = self.store.exclusive()?;
        core::set_git_identity(self.store.state_dir(), identity.as_ref())
    }
}

/// The Generate button's fixed settings.
#[uniffi::export]
pub fn default_password_policy() -> PasswordPolicy {
    PasswordPolicy::default()
}

/// A new random password.
#[uniffi::export]
pub fn generate_password(policy: PasswordPolicy) -> Result<String, WriteError> {
    Ok(generate::generate(&policy)?.as_str().to_owned())
}

/// How strong a password from `policy` is, in bits.
#[uniffi::export]
pub fn password_entropy_bits(policy: PasswordPolicy) -> Result<f64, WriteError> {
    Ok(generate::entropy_bits(&policy)?)
}

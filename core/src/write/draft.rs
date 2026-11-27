//! The add and edit form, before it is saved.
//!
//! A draft holds what the person typed as changes to make, not as a new
//! file: saving applies them to the opened entry's [`EntryDoc`], so only
//! the lines that changed are rewritten and every other byte (unknown
//! lines, their order, `otpauth://` lines, line endings) survives. A new
//! entry starts from an empty document and is laid out as design 2.8 says:
//! password, `login:`, `url:`, other fields, notes.
//!
//! Nothing is checked as it is typed; [`Draft::problems`] lists what
//! would stop a save, each with a sentence a person can act on.
//!
//! Words for people are the app's, with one exception: these sentences
//! ([`DraftProblem`], and the [`crate::otp::OtpProblem`] reasons they
//! include) are made here, beside the rules they explain, and shown as they
//! are.

use std::sync::{Arc, Mutex, MutexGuard};

use zeroize::Zeroizing;

use crate::entry::{EntryDoc, has_line_break, key_refusal, note_lines, value_refusal};
use crate::generate::{PasswordPolicy, PolicyError, generate};
use crate::path::{NameRefusal, folder_and_parents, folder_refusal, join, name_refusal, same_file, split};
use crate::store::PassStore;
use crate::store::tree::StoreTree;
use super::WriteError;

/// Which part of the form a problem is about.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum DraftField {
    Name,
    Folder,
    Password,
    Username,
    Url,
    /// One of the other `key: value` fields, by the key typed.
    Field { key: String },
    Notes,
    /// The one-time-code setup key or address.
    OneTimeCode,
}

/// Something that stops a save, worded for the person.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DraftProblem {
    pub field: DraftField,
    pub message: String,
}

fn problem(field: DraftField, message: impl Into<String>) -> DraftProblem {
    DraftProblem { field, message: message.into() }
}

enum FieldEdit {
    Set { key: String, value: Zeroizing<String> },
    Remove { key: String },
}

enum OtpEdit {
    /// What was typed or pasted: a base32 setup key or an `otpauth://`
    /// address.
    Set(Zeroizing<String>),
    Remove,
}

/// The entry being edited, as it was opened.
pub(crate) struct Original {
    pub(crate) path: String,
    pub(crate) doc: EntryDoc,
    /// The content id of the file it was opened from: saving is refused if
    /// the file has changed since.
    pub(crate) content_id: Vec<u8>,
}

pub struct Draft {
    original: Option<Original>,
    name: String,
    folder: String,
    password: Option<Zeroizing<String>>,
    generated: bool,
    username: Option<Zeroizing<String>>,
    url: Option<Zeroizing<String>>,
    fields: Vec<FieldEdit>,
    notes: Option<Zeroizing<String>>,
    otp: Option<OtpEdit>,
}

impl std::fmt::Debug for Draft {
    /// Names only; never a value.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Draft {{ path: {:?}, editing: {:?}, contents withheld }}",
            self.path(),
            self.original.as_ref().map(|o| &o.path)
        )
    }
}

fn place(folder: &str) -> String {
    if folder.is_empty() { "at the top level".into() } else { format!("in {folder}") }
}

pub(crate) fn name_problems(name: &str) -> Vec<DraftProblem> {
    let Some(refusal) = name_refusal(name) else { return Vec::new() };
    let message = match refusal {
        NameRefusal::Blank => "Give it a name.".into(),
        NameRefusal::Slash => "Names can't contain /.".into(),
        NameRefusal::LeadingDot => "Names can't start with a dot.".into(),
        NameRefusal::OuterSpace => "Names can't start or end with a space.".into(),
        NameRefusal::Control => "Names can't contain line breaks or other control characters.".into(),
        NameRefusal::Backslash => "Names can't contain \\.".into(),
        NameRefusal::TooLong => "That name is too long.".into(),
        NameRefusal::Unsafe(why) => format!("paw can't use this name: {why}."),
    };
    vec![problem(DraftField::Name, message)]
}

/// Problems with a folder path (`/` separated, empty for the top).
pub(crate) fn folder_problems(folder: &str) -> Vec<DraftProblem> {
    let Some(refusal) = folder_refusal(folder) else { return Vec::new() };
    let message = match refusal {
        NameRefusal::Blank => "A folder needs a name.".into(),
        NameRefusal::LeadingDot => "Folder names can't start with a dot.".into(),
        NameRefusal::OuterSpace => "Folder names can't start or end with a space.".into(),
        NameRefusal::Slash | NameRefusal::Control | NameRefusal::Backslash => {
            "Folder names can't contain \\, line breaks or other control characters.".into()
        }
        NameRefusal::TooLong => "That folder name is too long.".into(),
        NameRefusal::Unsafe(why) => format!("paw can't use this folder: {why}."),
    };
    vec![problem(DraftField::Folder, message)]
}

/// Whether `path` (an entry or folder, without `.gpg`) collides with
/// something in `tree` other than `except`, as a case-insensitive file
/// system would see it. Returns the problem to show.
fn collision(tree: &StoreTree, path: &str, except: Option<&str>) -> Option<DraftProblem> {
    let (folder, name) = split(path);
    if except.is_some_and(|except| same_file(except, path)) {
        return None;
    }
    if let Some(existing) = tree.entries().iter().find(|e| same_file(&e.path, path)) {
        return Some(problem(
            DraftField::Name,
            format!("There's already an entry called '{}' {}.", existing.name, place(folder)),
        ));
    }
    if tree_has_folder(tree, path) {
        return Some(problem(DraftField::Name, format!("There's already a folder called '{name}' {}.", place(folder))));
    }
    // A folder on the way spelled differently from one that exists would
    // be the same folder on the phone and two in git.
    let mut at = String::new();
    for component in folder.split('/').filter(|c| !c.is_empty()) {
        let parent = at.clone();
        at = join(&at, component);
        let listing = tree.folder(&parent);
        let other = listing.and_then(|l| {
            l.folders.iter().find(|f| same_file(&f.name, component) && f.name != component)
        });
        if let Some(other) = other {
            return Some(problem(
                DraftField::Folder,
                format!("There's already a folder called '{}' {}; use that name.", other.name, place(&parent)),
            ));
        }
    }
    None
}

fn tree_has_folder(tree: &StoreTree, path: &str) -> bool {
    tree.entries().iter().any(|entry| {
        folder_and_parents(&entry.folder).take_while(|folder| !folder.is_empty()).any(|folder| same_file(folder, path))
    })
}

fn single_line(field: DraftField, what: &str, value: &str) -> Option<DraftProblem> {
    has_line_break(value).then(|| problem(field, format!("{what} can't contain a line break.")))
}

fn value_problem(field: DraftField, what: &str, value: &str) -> Option<DraftProblem> {
    value_refusal(value).map(|refusal| problem(field, refusal.sentence(what)))
}

/// The problem with setting the field `key` to `value` (or removing it,
/// with `None`), as the form words it.
fn field_edit_problem(key: &str, value: Option<&str>) -> Option<DraftProblem> {
    let refusal = key_refusal(key).or_else(|| value.and_then(value_refusal))?;
    Some(problem(DraftField::Field { key: key.to_owned() }, refusal.sentence(&format!("The value of {}", key.trim()))))
}

impl Draft {
    pub fn new(folder: &str) -> Self {
        Self {
            original: None,
            name: String::new(),
            folder: folder.to_owned(),
            password: None,
            generated: false,
            username: None,
            url: None,
            fields: Vec::new(),
            notes: None,
            otp: None,
        }
    }

    /// An edit of the entry `path`, opened as `doc`. `content_id` is that
    /// of the file it was decrypted from ([`crate::files::content_id`]).
    pub fn edit(path: &str, doc: EntryDoc, content_id: Vec<u8>) -> Self {
        let (folder, name) = split(path);
        let mut draft = Self::new(folder);
        draft.name = name.to_owned();
        draft.original = Some(Original { path: path.to_owned(), doc, content_id });
        draft
    }

    /// The entry being edited, as it was opened; `None` for a new one.
    pub(crate) fn original(&self) -> Option<&Original> {
        self.original.as_ref()
    }

    /// Whether the password was made by the generator and not changed
    /// since. Decides the commit message for a new entry.
    pub fn generated(&self) -> bool {
        self.generated
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn folder(&self) -> &str {
        &self.folder
    }

    pub fn path(&self) -> String {
        join(&self.folder, &self.name)
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = name.to_owned();
    }

    /// `folder` is `/` separated; empty for the top. A folder that does not
    /// exist yet is made by saving.
    pub fn set_folder(&mut self, folder: &str) {
        self.folder = folder.trim_matches('/').to_owned();
    }

    pub fn set_password(&mut self, password: &str) {
        self.password = Some(Zeroizing::new(password.to_owned()));
        self.generated = false;
    }

    /// Generates a password with `policy`, puts it in the form and returns
    /// it to show.
    pub fn generate_password(&mut self, policy: &PasswordPolicy) -> Result<Zeroizing<String>, PolicyError> {
        let password = generate(policy)?;
        self.password = Some(password.clone());
        self.generated = true;
        Ok(password)
    }

    pub fn set_username(&mut self, username: &str) {
        self.username = Some(Zeroizing::new(username.to_owned()));
    }

    pub fn set_url(&mut self, url: &str) {
        self.url = Some(Zeroizing::new(url.to_owned()));
    }

    /// Sets the field `key` (any letter case), adding it if there is none.
    pub fn set_field(&mut self, key: &str, value: &str) {
        self.fields.push(FieldEdit::Set { key: key.to_owned(), value: Zeroizing::new(value.to_owned()) });
    }

    pub fn remove_field(&mut self, key: &str) {
        self.fields.push(FieldEdit::Remove { key: key.to_owned() });
    }

    pub fn set_notes(&mut self, notes: &str) {
        self.notes = Some(Zeroizing::new(notes.to_owned()));
    }

    /// Sets the one-time code from a base32 setup key or a pasted
    /// `otpauth://` address, replacing any the entry has. A key becomes the
    /// line pass-otp writes, labelled with the entry's name. Blank text
    /// undoes an earlier set or remove.
    pub fn set_one_time_code(&mut self, setup: &str) {
        self.otp = (!setup.trim().is_empty()).then(|| OtpEdit::Set(Zeroizing::new(setup.to_owned())));
    }

    pub fn remove_one_time_code(&mut self) {
        self.otp = Some(OtpEdit::Remove);
    }

    fn otp_line(&self, setup: &str) -> Result<Zeroizing<String>, DraftProblem> {
        crate::otp::setup_line(setup, &self.name).map_err(|why| problem(DraftField::OneTimeCode, why.reason()))
    }

    /// Problems with the form's values, without looking at the store.
    pub fn value_problems(&self) -> Vec<DraftProblem> {
        let mut problems = Vec::new();
        if let Some(p) = self.password.as_deref().and_then(|v| single_line(DraftField::Password, "The password", v)) {
            problems.push(p);
        }
        if let Some(p) = self.username.as_deref().and_then(|v| value_problem(DraftField::Username, "The username", v)) {
            problems.push(p);
        }
        if let Some(p) = self.url.as_deref().and_then(|v| value_problem(DraftField::Url, "The website", v)) {
            problems.push(p);
        }
        for edit in &self.fields {
            let found = match edit {
                FieldEdit::Set { key, value } => field_edit_problem(key, Some(value)),
                FieldEdit::Remove { key } => field_edit_problem(key, None),
            };
            problems.extend(found);
        }
        if let Some(notes) = &self.notes
            && note_lines(notes).is_none()
        {
            problems.push(problem(DraftField::Notes, "Notes can't contain a lone carriage return."));
        }
        if let Some(OtpEdit::Set(setup)) = &self.otp
            && let Err(p) = self.otp_line(setup)
        {
            problems.push(p);
        }
        problems
    }

    /// Everything that stops a save: the name and folder, the values, and
    /// (with `tree`, the store as it is now) a name that is taken.
    pub fn problems(&self, tree: Option<&StoreTree>) -> Vec<DraftProblem> {
        let mut problems = name_problems(&self.name);
        problems.extend(folder_problems(&self.folder));
        if problems.is_empty()
            && let Some(tree) = tree
            && let Some(taken) = collision(tree, &self.path(), self.original.as_ref().map(|o| o.path.as_str()))
        {
            problems.push(taken);
        }
        problems.extend(self.value_problems());
        problems
    }

    /// The text to save: the opened entry with the changes applied, or a
    /// new document. Fails with the problems if any value cannot be stored.
    pub fn build(&self) -> Result<EntryDoc, Vec<DraftProblem>> {
        let problems = self.value_problems();
        if !problems.is_empty() {
            return Err(problems);
        }
        let mut doc = match &self.original {
            Some(original) => original.doc.duplicate(),
            None => EntryDoc::empty(),
        };
        let fail = |field: DraftField| move |e: crate::PawError| vec![problem(field.clone(), e.to_string())];
        if let Some(password) = &self.password
            && password.as_str() != doc.password()
        {
            doc.set_password(password).map_err(fail(DraftField::Password))?;
        }
        if let Some(username) = &self.username
            && Some(username.as_str()) != doc.username()
        {
            doc.set_username(username).map_err(fail(DraftField::Username))?;
        }
        if let Some(url) = &self.url
            && Some(url.as_str()) != doc.url()
        {
            doc.set_url(url).map_err(fail(DraftField::Url))?;
        }
        for edit in &self.fields {
            match edit {
                FieldEdit::Set { key, value } => {
                    if doc.field(key.trim()) != Some(value.trim()) {
                        doc.set_field(key, value).map_err(fail(DraftField::Field { key: key.clone() }))?;
                    }
                }
                FieldEdit::Remove { key } => {
                    doc.remove_field(key.trim());
                }
            }
        }
        if let Some(notes) = &self.notes {
            doc.set_notes(notes).map_err(fail(DraftField::Notes))?;
        }
        match &self.otp {
            Some(OtpEdit::Set(setup)) => {
                let line = self.otp_line(setup).map_err(|p| vec![p])?;
                doc.set_one_time_code(&line).map_err(fail(DraftField::OneTimeCode))?;
            }
            Some(OtpEdit::Remove) => {
                doc.remove_one_time_code();
            }
            None => {}
        }
        Ok(doc)
    }

}

/// The add or edit form as Swift holds it: a [`Draft`] behind a lock. Make
/// one with [`super::StoreWriter::new_draft`] or
/// [`super::StoreWriter::edit_draft`].
#[derive(uniffi::Object)]
pub struct EntryDraft {
    inner: Mutex<Draft>,
}

impl EntryDraft {
    pub(crate) fn draft(&self) -> MutexGuard<'_, Draft> {
        // Each setter replaces one value whole.
        crate::locked(&self.inner)
    }

    pub(crate) fn wrap(draft: Draft) -> Arc<Self> {
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
    field_edit_problem(&key, Some(&value))
}

//! The add and edit form, before it is saved.
//!
//! A draft holds what the person typed as changes to make, not as a new
//! file: saving applies them to the opened entry's [`EntryDoc`], so only
//! the lines that changed are rewritten and every other byte (unknown
//! lines, their order, `otpauth://` lines, line endings) survives. A new
//! entry starts from an empty document and is laid out as design 2.8 says:
//! password, `login:`, `url:`, other fields, notes.
//!
//! Nothing is checked as it is typed; [`EntryDraft::problems`] lists what
//! would stop a save, each with a sentence a person can act on.

use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::entry::EntryDoc;
use crate::generate::{PasswordPolicy, PolicyError, generate};
use crate::path::unsafe_path;
use crate::store::tree::StoreTree;

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

/// A change to one of the other fields.
enum FieldEdit {
    Set { key: String, value: Zeroizing<String> },
    Remove { key: String },
}

/// The entry being edited, as it was opened.
struct Original {
    path: String,
    doc: EntryDoc,
    /// The content id of the file it was opened from, if known: saving is
    /// refused if the file has changed since.
    content_id: Option<Vec<u8>>,
}

/// An entry being added or edited.
pub struct EntryDraft {
    original: Option<Original>,
    name: String,
    folder: String,
    raw: Option<Zeroizing<String>>,
    password: Option<Zeroizing<String>>,
    generated: bool,
    username: Option<Zeroizing<String>>,
    url: Option<Zeroizing<String>>,
    fields: Vec<FieldEdit>,
    notes: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for EntryDraft {
    /// Names only; never a value.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "EntryDraft {{ path: {:?}, editing: {:?}, contents withheld }}",
            self.path(),
            self.original.as_ref().map(|o| &o.path)
        )
    }
}

/// `folder/name`, or `name` at the top.
pub(crate) fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() { name.to_owned() } else { format!("{folder}/{name}") }
}

/// `(folder, name)` of an entry path.
pub(crate) fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// How a file system that ignores letter case and Unicode normalisation
/// (APFS on iPhone, by default) sees a name.
pub(crate) fn canonical(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

fn place(folder: &str) -> String {
    if folder.is_empty() { "at the top level".into() } else { format!("in {folder}") }
}

/// Problems with one entry name, as the form words them.
pub(crate) fn name_problems(name: &str) -> Vec<DraftProblem> {
    let says = |message: &str| vec![problem(DraftField::Name, message)];
    if name.trim().is_empty() {
        return says("Give it a name.");
    }
    if name.contains('/') {
        return says("Names can't contain /.");
    }
    if name.starts_with('.') {
        return says("Names can't start with a dot.");
    }
    if name.trim() != name {
        return says("Names can't start or end with a space.");
    }
    if name.chars().any(char::is_control) {
        return says("Names can't contain line breaks or other control characters.");
    }
    if name.contains('\\') {
        return says("Names can't contain \\.");
    }
    if format!("{name}.gpg").len() > 255 {
        return says("That name is too long.");
    }
    if let Some(why) = unsafe_path(name) {
        return vec![problem(DraftField::Name, format!("paw can't use this name: {why}."))];
    }
    Vec::new()
}

/// Problems with a folder path (`/` separated, empty for the top).
pub(crate) fn folder_problems(folder: &str) -> Vec<DraftProblem> {
    if folder.is_empty() {
        return Vec::new();
    }
    let says = |message: String| vec![problem(DraftField::Folder, message)];
    for component in folder.split('/') {
        if component.trim().is_empty() {
            return says("A folder needs a name.".into());
        }
        if component.starts_with('.') {
            return says("Folder names can't start with a dot.".into());
        }
        if component.trim() != component {
            return says("Folder names can't start or end with a space.".into());
        }
        if component.chars().any(char::is_control) || component.contains('\\') {
            return says("Folder names can't contain \\, line breaks or other control characters.".into());
        }
        if component.len() > 255 {
            return says("That folder name is too long.".into());
        }
    }
    if let Some(why) = unsafe_path(folder) {
        return says(format!("paw can't use this folder: {why}."));
    }
    Vec::new()
}

/// Whether `path` (an entry or folder, without `.gpg`) collides with
/// something in `tree` other than `except`, as a case-insensitive file
/// system would see it. Returns the problem to show.
pub(crate) fn collision(tree: &StoreTree, path: &str, except: Option<&str>) -> Option<DraftProblem> {
    let (folder, name) = split(path);
    let wanted = canonical(path);
    let except = except.map(canonical);
    if except.as_deref() == Some(wanted.as_str()) {
        return None;
    }
    if let Some(existing) = tree.entries().iter().find(|e| canonical(&e.path) == wanted) {
        return Some(problem(
            DraftField::Name,
            format!("There's already an entry called '{}' {}.", existing.name, place(folder)),
        ));
    }
    if tree_has_folder(tree, &wanted) {
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
            l.folders.iter().find(|f| canonical(&f.name) == canonical(component) && f.name != component)
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

/// Whether some entry lives in a folder that is `canonical_path` once
/// letter case and normalisation are ignored.
fn tree_has_folder(tree: &StoreTree, canonical_path: &str) -> bool {
    tree.entries().iter().any(|entry| {
        let mut folder = entry.folder.as_str();
        while !folder.is_empty() {
            if canonical(folder) == canonical_path {
                return true;
            }
            folder = split(folder).0;
        }
        false
    })
}

fn line_break(text: &str) -> bool {
    text.contains(['\n', '\r'])
}

fn single_line(field: DraftField, what: &str, value: &str) -> Option<DraftProblem> {
    line_break(value).then(|| problem(field, format!("{what} can't contain a line break.")))
}

fn value_problem(field: DraftField, what: &str, value: &str) -> Option<DraftProblem> {
    single_line(field.clone(), what, value).or_else(|| {
        let trimmed = value.trim_start();
        let looks_otp = trimmed.get(..10).is_some_and(|s| s.eq_ignore_ascii_case("otpauth://"));
        (looks_otp || value.trim().starts_with("//"))
            .then(|| problem(field, format!("{what} can't start with // or be a one-time-code address.")))
    })
}

impl EntryDraft {
    /// A new entry in `folder`.
    pub fn new(folder: &str) -> Self {
        Self {
            original: None,
            name: String::new(),
            folder: folder.to_owned(),
            raw: None,
            password: None,
            generated: false,
            username: None,
            url: None,
            fields: Vec::new(),
            notes: None,
        }
    }

    /// An edit of the entry `path`, opened as `doc`. `content_id` is that
    /// of the file it was decrypted from ([`crate::copies::content_id`]),
    /// if known.
    pub fn edit(path: &str, doc: EntryDoc, content_id: Option<Vec<u8>>) -> Self {
        let (folder, name) = split(path);
        let mut draft = Self::new(folder);
        draft.name = name.to_owned();
        draft.original = Some(Original { path: path.to_owned(), doc, content_id });
        draft
    }

    /// The entry being edited, as it was opened; `None` for a new one.
    pub fn original_path(&self) -> Option<&str> {
        self.original.as_ref().map(|o| o.path.as_str())
    }

    pub(crate) fn original_doc(&self) -> Option<&EntryDoc> {
        self.original.as_ref().map(|o| &o.doc)
    }

    pub(crate) fn original_content_id(&self) -> Option<&[u8]> {
        self.original.as_ref().and_then(|o| o.content_id.as_deref())
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

    /// Where the entry will be saved, e.g. `Work/github.com`.
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

    /// Replaces the whole text (the "Text" toggle). Changes made with the
    /// other setters before this are dropped; later ones apply on top.
    pub fn set_text(&mut self, text: &str) {
        self.raw = Some(Zeroizing::new(text.to_owned()));
        self.password = None;
        self.username = None;
        self.url = None;
        self.fields.clear();
        self.notes = None;
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
            let (key, value) = match edit {
                FieldEdit::Set { key, value } => (key, Some(value)),
                FieldEdit::Remove { key } => (key, None),
            };
            let field = || DraftField::Field { key: key.clone() };
            if key.trim().is_empty() {
                problems.push(problem(field(), "Give the field a name."));
            } else if line_break(key) {
                problems.push(problem(field(), "A field name can't contain a line break."));
            } else if key.contains(':') {
                problems.push(problem(field(), "A field name can't contain a colon."));
            } else if let Some(p) = value.and_then(|v| value_problem(field(), &format!("The value of {}", key.trim()), v)) {
                problems.push(p);
            }
        }
        if let Some(notes) = &self.notes
            && notes.replace("\r\n", "\n").contains('\r')
        {
            problems.push(problem(DraftField::Notes, "Notes can't contain a lone carriage return."));
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
            && let Some(taken) = collision(tree, &self.path(), self.original_path())
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
        let mut doc = match (&self.raw, &self.original) {
            (Some(raw), _) => {
                let mut bytes = Zeroizing::new(raw.as_bytes().to_vec());
                EntryDoc::parse(std::mem::take(&mut *bytes))
                    .map_err(|_| vec![problem(DraftField::Notes, "The text isn't valid.")])?
            }
            (None, Some(original)) => original.doc.duplicate(),
            (None, None) => EntryDoc::empty(),
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
            doc.set_notes(&notes.replace("\r\n", "\n")).map_err(fail(DraftField::Notes))?;
        }
        Ok(doc)
    }

    /// The text as it would be saved, for the form to show; the opened
    /// entry (or an empty one) if a value cannot be stored yet.
    pub fn preview(&self) -> EntryDoc {
        self.build().unwrap_or_else(|_| {
            self.original.as_ref().map_or_else(EntryDoc::empty, |o| o.doc.duplicate())
        })
    }
}

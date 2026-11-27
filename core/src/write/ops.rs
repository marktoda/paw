//! Saving, deleting, renaming and moving entries, each as commits of one
//! entry in `pass`'s own words:
//!
//! | change | message |
//! |---|---|
//! | add, generated password | `Add generated password for <path>.` |
//! | add, typed password | `Add given password for <path> to store.` |
//! | edit | `Edit password for <path> using paw.` |
//! | delete | `Remove <path> from store.` |
//! | rename or move | `Rename <old> to <new>.` |
//!
//! A rename or a move between folders with the same recipients keeps the
//! encrypted file as it is, so it needs no plaintext. A move to a folder
//! whose `.gpg-id` names other keys re-encrypts, as `pass mv` does, which
//! needs the entry open. Folders appear with their first entry and go with
//! their last, as in git.
//!
//! After its commits each write keeps paw's own records of the entry in
//! step ([`FollowUp`]). The commits stand whatever happens to those; what
//! failed is returned with the outcome.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::draft::{EntryDraft, folder_problems, name_problems};
use super::keyring::{FolderRecipients, Keyring};
use super::{WriteError, author, encrypt};
use crate::PawError;
use crate::copies::CopyCache;
use crate::fault::Fault;
use crate::files::content_id;
use crate::entry::EntryDoc;
use crate::entry::opened::OpenedEntry;
use crate::store::codes::OneTimeCodeEntries;
use crate::store::pairings::SitePairings;
use crate::store::gpg_id::recipients_for;
use crate::store::read::read_file;
use crate::store::tree::StoreTree;
use crate::lock::{Exclusive, StoreLock};
use crate::path::{entry_file as file, folder_of, split};
use crate::sync::{TreeEdit, commit_edits};

/// What a write keeps in step with the entry after its commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FollowUp {
    /// The sites the entry was filled on follow it when it moves or goes.
    SitePairings,
    /// Whether the entry has a one-time code, for AutoFill.
    OneTimeCodeMark,
    /// The entry's Face ID copy: made, resealed with the new text, or
    /// forgotten.
    FaceIdCopy,
}

/// A follow-up that failed. The write it followed stands.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FollowUpFailure {
    pub follow_up: FollowUp,
    pub message: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Saved {
    pub entry: String,
    /// Where it was, if the write moved it.
    pub previous: Option<String>,
    /// The content id of its file now ([`crate::files::content_id`]).
    pub content_id: Vec<u8>,
    /// The commits made, oldest first: none if nothing changed.
    pub commits: Vec<String>,
    /// The entry as saved, open. `None` for a rename or move.
    pub opened: Option<Arc<OpenedEntry>>,
    /// Until when the entry can be opened by Face ID, if it has a copy.
    pub face_id_until_ms: Option<i64>,
    pub follow_up_failures: Vec<FollowUpFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Deleted {
    pub commit: String,
    pub follow_up_failures: Vec<FollowUpFailure>,
}

/// The write side of a store: its checkout and its state directory (where
/// the pinned keys, the git identity, the site pairings and the one-time
/// code marks are), and the Face ID copies its writes keep in step.
#[derive(Clone)]
pub struct WriteStore {
    state_dir: PathBuf,
    repo: PathBuf,
    copies: Option<Arc<CopyCache>>,
    new_copy_ttl: Duration,
}

struct Committed {
    entry: String,
    previous: Option<String>,
    content_id: Vec<u8>,
    commits: Vec<String>,
    doc: EntryDoc,
}

/// Collects the follow-ups that failed.
#[derive(Default)]
struct FollowUps(Vec<FollowUpFailure>);

impl FollowUps {
    fn io(&mut self, follow_up: FollowUp, done: std::io::Result<impl Sized>) {
        if let Err(e) = done {
            self.0.push(FollowUpFailure { follow_up, message: e.to_string() });
        }
    }

    fn copy(&mut self, done: Result<Option<i64>, PawError>) -> Option<i64> {
        done.unwrap_or_else(|e| {
            self.0.push(FollowUpFailure { follow_up: FollowUp::FaceIdCopy, message: e.to_string() });
            None
        })
    }
}

/// Refuses a path the add form would refuse: hidden names (so `.gpg-id`
/// and anything under a dot-folder), separators, unsafe names.
fn check_path(entry: &str) -> Result<(), WriteError> {
    let (folder, name) = split(entry);
    let mut problems = name_problems(name);
    problems.extend(folder_problems(folder));
    if problems.is_empty() { Ok(()) } else { Err(WriteError::Invalid { problems }) }
}

impl WriteStore {
    /// The store set up in `state_dir` (checked out in its `repo/`).
    pub fn open(state_dir: &Path) -> Self {
        Self::new(state_dir, &crate::files::layout::repo_dir(state_dir))
    }

    pub fn new(state_dir: &Path, repo: &Path) -> Self {
        Self { state_dir: state_dir.to_path_buf(), repo: repo.to_path_buf(), copies: None, new_copy_ttl: Duration::ZERO }
    }

    /// The same store, keeping the Face ID copies in `copies` in step: an
    /// edited or moved entry's live copy is resealed with its new text for
    /// the time it had left, a deleted or renamed one is forgotten, and a
    /// new entry gets one for `new_copy_ttl` (zero keeps none).
    pub fn with_copies(&self, copies: Option<Arc<CopyCache>>, new_copy_ttl: Duration) -> Self {
        Self { copies, new_copy_ttl, ..self.clone() }
    }

    /// The entry moved from `from` to `to`, or with `None` went: its site
    /// pairings and its one-time-code mark follow it.
    fn entry_moved(&self, from: &str, to: Option<&str>, follow_ups: &mut FollowUps) {
        let pairings = SitePairings::new(&self.state_dir);
        let moved = match to {
            Some(to) => pairings.moved(from, to),
            None => pairings.clear(Some(from)),
        };
        follow_ups.io(FollowUp::SitePairings, moved);
        follow_ups.io(FollowUp::OneTimeCodeMark, OneTimeCodeEntries::new(&self.state_dir).moved(from, to));
    }

    fn forget_copy(&self, entry: &str, follow_ups: &mut FollowUps) {
        if let Some(copies) = &self.copies {
            follow_ups.copy(copies.forget_entry(entry.to_owned()).map(|()| None));
        }
    }

    pub fn repo(&self) -> &Path {
        &self.repo
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// The store's lock (see [`crate::lock`]). Every change here takes it
    /// for its whole check-then-commit, so two saves never interleave and a
    /// check is never stale by the time its commit is made.
    pub fn lock(&self) -> StoreLock {
        StoreLock::for_repo(&self.repo)
    }

    pub fn exclusive(&self) -> Result<Exclusive, WriteError> {
        Ok(self.lock().exclusive()?)
    }

    pub fn keyring(&self) -> Result<Keyring, Fault> {
        Keyring::open(&self.state_dir)
    }

    fn tree(&self) -> Result<StoreTree, WriteError> {
        Ok(StoreTree::scan(&self.repo)?)
    }

    /// The encrypted bytes of `entry`.
    fn current(&self, entry: &str) -> Result<Vec<u8>, WriteError> {
        check_path(entry)?;
        read_file(&self.repo, &file(entry))?.ok_or_else(|| WriteError::NoSuchEntry { entry: entry.to_owned() })
    }

    /// Whether entries in the two folders are encrypted for the same
    /// `.gpg-id` lines (in any order).
    fn same_recipients(&self, a: &str, b: &str) -> Result<bool, WriteError> {
        let lines = |folder: &str| -> Result<Vec<String>, WriteError> {
            let mut lines = recipients_for(&self.repo, folder)?;
            lines.sort();
            lines.dedup();
            Ok(lines)
        };
        Ok(lines(a)? == lines(b)?)
    }

    /// How the recipients for `folder` are satisfied; `writable` says
    /// whether paw can save there now.
    pub fn folder_recipients(&self, folder: &str) -> Result<FolderRecipients, WriteError> {
        let _reading = self.lock().shared()?;
        Ok(self.keyring()?.folder_recipients(&self.repo, folder)?)
    }

    /// Every `.gpg-id` in the store, top first, with its keys and the
    /// entries under it encrypted for a key it does not name. Reads headers
    /// only; decrypts nothing.
    pub fn recipient_scopes(&self) -> Result<Vec<super::RecipientScope>, WriteError> {
        let _reading = self.lock().shared()?;
        super::scopes::recipient_scopes(&self.keyring()?, &self.repo)
    }

    /// After "Who can open … changed": accepts the folder's current
    /// `.gpg-id`.
    pub fn accept_recipients(&self, folder: &str) -> Result<(), WriteError> {
        // Each keyring change reads, changes and writes the whole file: one
        // critical section, or two changes at once would lose one.
        let held = self.exclusive()?;
        self.keyring()?.accept_recipients(&held, &self.repo, folder)
    }

    /// Saves `draft`: encrypts to every recipient of its folder and commits.
    /// Returns what was done, with the entry as saved, open. No key, PIN or
    /// network is involved.
    pub fn save(&self, draft: &EntryDraft) -> Result<Saved, WriteError> {
        let Committed { entry, previous, content_id, commits, doc } = self.save_commits(draft)?;
        let mut follow_ups = FollowUps::default();
        if let Some(previous) = &previous {
            self.entry_moved(previous, Some(&entry), &mut follow_ups);
        }
        // AutoFill learns of a code added or removed here without the
        // entry being opened again.
        follow_ups.io(FollowUp::OneTimeCodeMark, OneTimeCodeEntries::new(&self.state_dir).mark(&entry, doc.makes_codes()));
        let face_id_until_ms = self.copies.as_ref().and_then(|copies| {
            let text = doc.to_bytes();
            follow_ups.copy(match draft.original_path() {
                None => copies.put(&entry, &text, &content_id, self.new_copy_ttl),
                Some(original) => copies.reseal(original, &entry, &text, &content_id),
            })
        });
        Ok(Saved {
            opened: Some(OpenedEntry::new(doc, content_id.clone())),
            entry,
            previous,
            content_id,
            commits,
            face_id_until_ms,
            follow_up_failures: follow_ups.0,
        })
    }

    fn save_commits(&self, draft: &EntryDraft) -> Result<Committed, WriteError> {
        let held = self.exclusive()?;
        let held = &held;
        let tree = self.tree()?;
        let problems = draft.problems(Some(&tree));
        if !problems.is_empty() {
            return Err(WriteError::Invalid { problems });
        }
        let doc = draft.build().map_err(|problems| WriteError::Invalid { problems })?;
        let target = draft.path();
        let folder = folder_of(&target);
        let author = author();
        let keyring = self.keyring()?;

        let Some(original) = draft.original_path() else {
            if read_file(&self.repo, &file(&target))?.is_some() {
                return Err(WriteError::AlreadyExists { entry: target });
            }
            let keys = keyring.resolve(&self.repo, folder)?;
            let ciphertext = encrypt(&keys, &doc.to_bytes())?;
            let message = if draft.generated() {
                format!("Add generated password for {target}.")
            } else {
                format!("Add given password for {target} to store.")
            };
            let commit = commit_edits(held, &self.repo, &[TreeEdit::Write { path: &file(&target), contents: &ciphertext }], &message, &author)?;
            return Ok(Committed { entry: target, previous: None, content_id: content_id(&ciphertext), commits: vec![commit], doc });
        };

        let current = self.current(original)?;
        if draft.original_content_id().is_some_and(|id| id != content_id(&current)) {
            return Err(WriteError::ChangedSinceOpened { entry: original.to_owned() });
        }
        let opened = draft.original_doc().ok_or_else(|| WriteError::NoSuchEntry { entry: original.to_owned() })?;
        let changed = *doc.to_bytes() != *opened.to_bytes();
        let moved = target != original;
        let reencrypt = moved && !self.same_recipients(folder_of(original), folder)?;
        // Everything that can be refused is asked before the first commit.
        let keys = if changed || reencrypt { Some(keyring.resolve(&self.repo, folder)?) } else { None };

        let mut commits = Vec::new();
        let mut latest = current;
        if moved {
            let (commit, ciphertext) = self.move_commit(held, original, &target, reencrypt.then_some((opened, keys.as_deref().unwrap_or_default())), &author)?;
            commits.push(commit);
            if let Some(ciphertext) = ciphertext {
                latest = ciphertext;
            }
        }
        if changed && let Some(keys) = &keys {
            let ciphertext = encrypt(keys, &doc.to_bytes())?;
            let message = format!("Edit password for {target} using paw.");
            commits.push(commit_edits(held, &self.repo, &[TreeEdit::Write { path: &file(&target), contents: &ciphertext }], &message, &author)?);
            latest = ciphertext;
        }
        Ok(Committed { entry: target, previous: moved.then(|| original.to_owned()), content_id: content_id(&latest), commits, doc })
    }

    /// Commits `from` -> `to`: the same file, or with `reencrypt` the
    /// opened text encrypted for `keys`. Returns the commit and any new
    /// ciphertext.
    fn move_commit(
        &self,
        held: &Exclusive,
        from: &str,
        to: &str,
        reencrypt: Option<(&EntryDoc, &[pgp::packet::PublicKey])>,
        author: &crate::sync::Author,
    ) -> Result<(String, Option<Vec<u8>>), WriteError> {
        let message = format!("Rename {from} to {to}.");
        let (from_file, to_file) = (file(from), file(to));
        match reencrypt {
            None => {
                let commit = commit_edits(held, &self.repo, &[TreeEdit::Rename { from: &from_file, to: &to_file }], &message, author)?;
                Ok((commit, None))
            }
            Some((doc, keys)) => {
                let ciphertext = encrypt(keys, &doc.to_bytes())?;
                let edits = [TreeEdit::Remove { path: &from_file }, TreeEdit::Write { path: &to_file, contents: &ciphertext }];
                let commit = commit_edits(held, &self.repo, &edits, &message, author)?;
                Ok((commit, Some(ciphertext)))
            }
        }
    }

    /// Deletes `entry` (it stays in the git history). A folder left empty
    /// goes too.
    pub fn delete(&self, entry: &str) -> Result<Deleted, WriteError> {
        let commit = {
            let held = self.exclusive()?;
            self.current(entry)?;
            let message = format!("Remove {entry} from store.");
            commit_edits(&held, &self.repo, &[TreeEdit::Remove { path: &file(entry) }], &message, &author())?
        };
        let mut follow_ups = FollowUps::default();
        self.forget_copy(entry, &mut follow_ups);
        self.entry_moved(entry, None, &mut follow_ups);
        Ok(Deleted { commit, follow_up_failures: follow_ups.0 })
    }

}

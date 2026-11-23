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

use std::path::{Path, PathBuf};

use super::draft::{EntryDraft, collision, folder_problems, name_problems, split};
use super::keyring::{FolderRecipients, Keyring};
use super::{WriteError, author, encrypt, now_secs};
use crate::copies::content_id;
use crate::entry::EntryDoc;
use crate::store::gpg_id::recipients_for;
use crate::store::read::read_file;
use crate::store::tree::StoreTree;
use crate::lock::{Exclusive, StoreLock};
use crate::sync::{TreeEdit, commit_edits};

/// What a save did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    /// Where the entry is now.
    pub entry: String,
    /// Where it was, if the save moved it.
    pub previous: Option<String>,
    /// The content id of its file now ([`crate::copies::content_id`]), for
    /// refreshing a Face ID copy.
    pub content_id: Vec<u8>,
    /// The commits made, oldest first: none if nothing changed.
    pub commits: Vec<String>,
}

/// The write side of a store: its checkout and its state directory (where
/// the pinned keys and the git identity are).
pub struct WriteStore {
    state_dir: PathBuf,
    repo: PathBuf,
}

fn file(entry: &str) -> String {
    format!("{entry}.gpg")
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
        Self::new(state_dir, &crate::setup::repo_dir(state_dir))
    }

    /// A store checked out at `repo` with its state in `state_dir`.
    pub fn new(state_dir: &Path, repo: &Path) -> Self {
        Self { state_dir: state_dir.to_path_buf(), repo: repo.to_path_buf() }
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

    /// Takes the store's lock for changing it.
    pub fn exclusive(&self) -> Result<Exclusive, WriteError> {
        Ok(self.lock().exclusive()?)
    }

    pub fn keyring(&self) -> Result<Keyring, WriteError> {
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
            let mut lines: Vec<String> = recipients_for(&self.repo, folder)?.iter().map(|l| l.trim().to_owned()).collect();
            lines.sort();
            lines.dedup();
            Ok(lines)
        };
        Ok(lines(a)? == lines(b)?)
    }

    /// How the recipients for `folder` are satisfied; `writable()` says
    /// whether paw can save there now.
    pub fn folder_recipients(&self, folder: &str) -> Result<FolderRecipients, WriteError> {
        self.keyring()?.folder_recipients(&self.repo, folder, now_secs())
    }

    /// Saves `draft`: encrypts to every recipient of its folder and commits.
    /// Returns what was done and the text saved, for refreshing a Face ID
    /// copy. No key, PIN or network is involved.
    pub fn save(&self, draft: &EntryDraft) -> Result<(Saved, EntryDoc), WriteError> {
        let held = self.exclusive()?;
        let held = &held;
        let tree = self.tree()?;
        let problems = draft.problems(Some(&tree));
        if !problems.is_empty() {
            return Err(WriteError::Invalid { problems });
        }
        let doc = draft.build().map_err(|problems| WriteError::Invalid { problems })?;
        let target = draft.path();
        let (folder, _) = split(&target);
        let author = author(&self.state_dir);
        let keyring = self.keyring()?;
        let now = now_secs();

        let Some(original) = draft.original_path() else {
            if read_file(&self.repo, &file(&target))?.is_some() {
                return Err(WriteError::AlreadyExists { entry: target });
            }
            let keys = keyring.resolve(&self.repo, folder, now)?;
            let ciphertext = encrypt(&keys, &doc.to_bytes())?;
            let message = if draft.generated() {
                format!("Add generated password for {target}.")
            } else {
                format!("Add given password for {target} to store.")
            };
            let commit = commit_edits(held, &self.repo, &[TreeEdit::Write { path: &file(&target), contents: &ciphertext }], &message, &author)?;
            let saved = Saved { entry: target, previous: None, content_id: content_id(&ciphertext), commits: vec![commit] };
            return Ok((saved, doc));
        };

        let current = self.current(original)?;
        if draft.original_content_id().is_some_and(|id| id != content_id(&current)) {
            return Err(WriteError::ChangedSinceOpened { entry: original.to_owned() });
        }
        let opened = draft.original_doc().ok_or_else(|| WriteError::NoSuchEntry { entry: original.to_owned() })?;
        let changed = *doc.to_bytes() != *opened.to_bytes();
        let moved = target != original;
        let reencrypt = moved && !self.same_recipients(split(original).0, folder)?;
        // Everything that can be refused is asked before the first commit.
        let keys = if changed || reencrypt { Some(keyring.resolve(&self.repo, folder, now)?) } else { None };

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
        let saved = Saved {
            entry: target,
            previous: moved.then(|| original.to_owned()),
            content_id: content_id(&latest),
            commits,
        };
        Ok((saved, doc))
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

    /// Deletes `entry`. Returns the commit. A folder left empty goes too.
    pub fn delete(&self, entry: &str) -> Result<String, WriteError> {
        let held = self.exclusive()?;
        self.current(entry)?;
        let message = format!("Remove {entry} from store.");
        Ok(commit_edits(&held, &self.repo, &[TreeEdit::Remove { path: &file(entry) }], &message, &author(&self.state_dir))?)
    }

    /// Renames or moves `from` to `to` (full paths). Within the same
    /// recipients the encrypted file moves unchanged and `open` is not
    /// needed. To a folder with other recipients the entry is re-encrypted
    /// from `open` (its decrypted text), and without it the move is refused
    /// with [`WriteError::NeedsOpenEntry`]. Returns what was done.
    pub fn rename(&self, from: &str, to: &str, open: Option<&EntryDoc>) -> Result<Saved, WriteError> {
        let held = self.exclusive()?;
        let current = self.current(from)?;
        check_path(to)?;
        if from == to {
            return Ok(Saved { entry: to.to_owned(), previous: None, content_id: content_id(&current), commits: Vec::new() });
        }
        if let Some(taken) = collision(&self.tree()?, to, Some(from)) {
            return Err(WriteError::Invalid { problems: vec![taken] });
        }
        let to_folder = split(to).0;
        let reencrypt = !self.same_recipients(split(from).0, to_folder)?;
        let keys = match (reencrypt, open) {
            (false, _) => None,
            (true, None) => return Err(WriteError::NeedsOpenEntry { entry: from.to_owned() }),
            (true, Some(_)) => Some(self.keyring()?.resolve(&self.repo, to_folder, now_secs())?),
        };
        let reencrypt = open.zip(keys.as_deref());
        let (commit, ciphertext) = self.move_commit(&held, from, to, reencrypt, &author(&self.state_dir))?;
        Ok(Saved {
            entry: to.to_owned(),
            previous: Some(from.to_owned()),
            content_id: content_id(ciphertext.as_deref().unwrap_or(&current)),
            commits: vec![commit],
        })
    }
}

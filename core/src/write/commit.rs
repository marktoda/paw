//! Committing one change made on this device: what every add, edit,
//! delete and move writes.

use std::path::Path;

use git2::FileMode;

use crate::PawError;
use crate::fault::Fault;
use crate::lock::Exclusive;
use crate::path::{unsafe_path, unsafe_path_err};
use crate::sync::{OpenStore, git_err, guard, history};

/// Who a commit made on this device is attributed to.
pub struct Author {
    pub name: String,
    pub email: String,
}

/// One change to one file in a commit made by [`commit_edits`].
pub(crate) enum TreeEdit<'a> {
    Write { path: &'a str, contents: &'a [u8] },
    /// Remove the file `path`, which must be a regular file in the tree.
    Remove { path: &'a str },
    /// Move the file `from` to `to` unchanged: the same blob, so git and
    /// the replay see a rename.
    Rename { from: &'a str, to: &'a str },
}

impl TreeEdit<'_> {
    fn paths(&self) -> Vec<&str> {
        match self {
            Self::Write { path, .. } | Self::Remove { path } => vec![path],
            Self::Rename { from, to } => vec![from, to],
        }
    }
}

/// Checks `path` in the working copy before anything is written: no link
/// on the way and no file where a folder should be. The working copy can
/// differ from the tree, and checkout does follow a folder link it finds
/// there (seen with libgit2 1.9). `symlink_metadata` does not follow links.
fn check_disk_path(dir: &Path, path: &str) -> Result<(), PawError> {
    let mut on_disk = dir.to_path_buf();
    let mut components = path.split('/').peekable();
    while let Some(component) = components.next() {
        on_disk.push(component);
        let is_folder = components.peek().is_some();
        match std::fs::symlink_metadata(&on_disk) {
            Ok(found) if found.is_symlink() || (is_folder && !found.is_dir()) => {
                let why = "a link or a file in the working copy is in its way";
                return Err(unsafe_path_err(path, why));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Commits `edits` on the current branch as one commit, and changes
/// nothing else.
///
/// The commit is built from the branch's tree, not from the working copy,
/// and the files reach the working copy through libgit2's checkout. A
/// cloned repository is not trusted and may contain symbolic links, so a
/// path that runs through one, in the tree or on disk, is refused with
/// [`PawError::UnsafePath`] (or [`Fault::Refused`], for a name the tree
/// refuses) before anything is written.
///
/// `message` is the subject line, in `pass`'s own wording
/// ("Add given password for web/example to store.") so the history reads the
/// same whichever tool wrote it. `held` is the store's lock: the whole
/// commit, from reading `HEAD` to the checkout, is one critical section.
pub(crate) fn commit_edits(
    held: &Exclusive,
    store: &OpenStore,
    edits: &[TreeEdit<'_>],
    message: &str,
    author: &Author,
) -> Result<String, Fault> {
    debug_assert!(held.covers(store.dirs.lock()), "commit_edits without its store's lock");
    for path in edits.iter().flat_map(TreeEdit::paths) {
        if let Some(why) = unsafe_path(path) {
            return Err(unsafe_path_err(path, why).into());
        }
    }
    let repo = &store.repo;
    let parent = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(git_err)?;
    let base = parent.tree().map_err(git_err)?;

    for path in edits.iter().flat_map(TreeEdit::paths) {
        history::check_tree_path(&base, path)?;
        check_disk_path(store.dirs.repo_dir(), path)?;
    }

    let mut index = git2::Index::new().map_err(git_err)?;
    index.read_tree(&base).map_err(git_err)?;
    for edit in edits {
        let existing = |path: &str| {
            base.get_path(Path::new(path))
                .map(|entry| entry.id())
                .map_err(|_| PawError::Store(format!("there is no file called {path}")))
        };
        match edit {
            TreeEdit::Write { path, contents } => {
                let blob = repo.blob(contents).map_err(git_err)?;
                index
                    .add(&history::index_entry(path, blob, FileMode::Blob.into()))
                    .map_err(git_err)?;
            }
            TreeEdit::Remove { path } => {
                existing(path)?;
                index.remove_path(Path::new(path)).map_err(git_err)?;
            }
            TreeEdit::Rename { from, to } => {
                let blob = existing(from)?;
                index.remove_path(Path::new(from)).map_err(git_err)?;
                index
                    .add(&history::index_entry(to, blob, FileMode::Blob.into()))
                    .map_err(git_err)?;
            }
        }
    }
    let tree_id = index.write_tree_to(repo).map_err(git_err)?;
    // A new name can collide with one already there (`Web/x` beside `web/`).
    guard::refuse(repo, tree_id)?;
    let tree = repo.find_tree(tree_id).map_err(git_err)?;

    let signature = git2::Signature::now(&author.name, &author.email).map_err(git_err)?;
    let id = repo
        .commit(Some("HEAD"), &signature, &signature, message, &tree, &[&parent])
        .map_err(git_err)?;
    // The names are taken literally. Left to itself, checkout reads them as
    // patterns, and an entry called `web/*` would overwrite its neighbours.
    let mut only_these_files = guard::checkout();
    only_these_files.disable_pathspec_match(true);
    for path in edits.iter().flat_map(TreeEdit::paths) {
        only_these_files.path(path);
    }
    repo.checkout_head(Some(&mut only_these_files)).map_err(git_err)?;
    Ok(id.to_string())
}


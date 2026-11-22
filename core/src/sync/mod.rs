//! Keeping the local copy of the password store in step with the git host.
//!
//! This is the whole of paw's use of libgit2: [`clone`], [`fetch`],
//! [`commit`] and [`push`]. The network ones take a [`Connection`], which the
//! caller opens once and may use for several of them; see `transport` for
//! why that matters.
//!
//! A push only ever fast-forwards. If the remote has moved, [`push`] fails
//! with [`PawError::NotFastForward`] and changes nothing.

mod transport;

use std::path::Path;

use git2::build::CheckoutBuilder;
use git2::{
    AutotagOption, Direction, FetchOptions, FileMode, ObjectType, PushOptions, RemoteCallbacks,
    Repository,
};

pub use transport::{Connection, HostKeyPin, Remote};

use crate::PawError;
use crate::path::{unsafe_path, unsafe_path_err};

/// Every branch of the remote is mirrored under `origin/`.
const FETCH_REFSPEC: &str = "+refs/heads/*:refs/remotes/origin/*";

/// Who a commit made on this device is attributed to.
pub struct Author {
    pub name: String,
    pub email: String,
}

fn git_err(e: git2::Error) -> PawError {
    PawError::Git(e.message().to_owned())
}

/// Fetches all branches and returns the remote's default branch
/// (`refs/heads/...`), if it has one.
fn fetch_branches(connection: &Connection, repo: &Repository) -> Result<Option<String>, PawError> {
    connection.with_remote(repo, |remote| {
        let mut connected = remote.connect_auth(Direction::Fetch, None, None)?;
        // An empty repository has no default branch; that is not an error.
        let default = connected
            .default_branch()
            .ok()
            .and_then(|name| name.as_str().map(str::to_owned).ok());
        let mut options = FetchOptions::new();
        options.download_tags(AutotagOption::None);
        connected
            .remote()
            .fetch(&[FETCH_REFSPEC], Some(&mut options), None)?;
        Ok(default)
    })
}

/// The branch `HEAD` points at, as `(refs/heads/<name>, <name>)`.
fn current_branch(repo: &Repository) -> Result<(String, String), PawError> {
    let head = repo.find_reference("HEAD").map_err(git_err)?;
    let not_on_branch = || PawError::Git("HEAD is not on a branch".into());
    let full = head.symbolic_target().map_err(git_err)?.ok_or_else(not_on_branch)?;
    let short = full.strip_prefix("refs/heads/").ok_or_else(not_on_branch)?;
    Ok((full.to_owned(), short.to_owned()))
}

/// Clones the remote's default branch into `dir`, which must be empty or
/// not exist yet.
pub fn clone(connection: &Connection, remote: &Remote, dir: &Path) -> Result<(), PawError> {
    let repo = Repository::init(dir).map_err(git_err)?;
    // Recorded so the copy is readable with ordinary git tools. paw itself
    // always goes through a `Connection`.
    repo.remote("origin", &remote.url()).map_err(git_err)?;

    let branch = fetch_branches(connection, &repo)?
        .ok_or_else(|| PawError::Git("the remote repository is empty".into()))?;
    let short = branch
        .strip_prefix("refs/heads/")
        .ok_or_else(|| PawError::Git(format!("unexpected default branch {branch}")))?;
    let tip = repo
        .refname_to_id(&format!("refs/remotes/origin/{short}"))
        .map_err(git_err)?;

    repo.reference(&branch, tip, false, "clone").map_err(git_err)?;
    repo.set_head(&branch).map_err(git_err)?;
    repo.checkout_head(Some(CheckoutBuilder::new().force()))
        .map_err(git_err)
}

/// Updates `origin/*` from the remote and returns the new tip of the current
/// branch's remote counterpart. The working copy is not changed.
pub fn fetch(connection: &Connection, dir: &Path) -> Result<String, PawError> {
    let repo = Repository::open(dir).map_err(git_err)?;
    fetch_branches(connection, &repo)?;
    let (_, short) = current_branch(&repo)?;
    repo.refname_to_id(&format!("refs/remotes/origin/{short}"))
        .map(|id| id.to_string())
        .map_err(git_err)
}

/// Commits `contents` as the file `path` (relative to the store, `/`
/// separated) on the current branch, and nothing else. Returns the commit id.
///
/// The commit is built from the branch's tree, not from the working copy,
/// and the file reaches the working copy through libgit2's checkout. A
/// cloned repository is not trusted and may contain symbolic links, so a
/// path that runs through one, in the tree or on disk, is refused with
/// [`PawError::UnsafePath`] before anything is written.
///
/// `message` is the subject line, in `pass`'s own wording
/// ("Add given password for web/example to store.") so the history reads the
/// same whichever tool wrote it.
pub fn commit(
    dir: &Path,
    path: &str,
    contents: &[u8],
    message: &str,
    author: &Author,
) -> Result<String, PawError> {
    if let Some(why) = unsafe_path(path) {
        return Err(unsafe_path_err(path, why));
    }
    let repo = Repository::open(dir).map_err(git_err)?;
    let parent = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(git_err)?;
    let base = parent.tree().map_err(git_err)?;

    // Every folder on the way must be a real folder in the tree (or not
    // exist yet), and the entry itself a regular file (or not exist yet).
    let folders: Vec<usize> = path.match_indices('/').map(|(at, _)| at).collect();
    for end in folders {
        match base.get_path(Path::new(&path[..end])) {
            Ok(entry) if entry.kind() != Some(ObjectType::Tree) => {
                let why = "one of its folders is a link or a file in the repository";
                return Err(unsafe_path_err(path, why));
            }
            _ => {}
        }
    }
    if let Ok(entry) = base.get_path(Path::new(path)) {
        let regular = [FileMode::Blob, FileMode::BlobExecutable].map(i32::from);
        if !regular.contains(&entry.filemode()) {
            return Err(unsafe_path_err(path, "it is a link or a folder in the repository"));
        }
    }

    // The working copy can differ from the tree, and checkout does follow a
    // folder link it finds there (seen with libgit2 1.9), so look before
    // anything is changed. `symlink_metadata` does not follow links.
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

    let mut index = git2::Index::new().map_err(git_err)?;
    index.read_tree(&base).map_err(git_err)?;
    index
        .add(&git2::IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: FileMode::Blob.into(),
            uid: 0,
            gid: 0,
            file_size: 0,
            id: repo.blob(contents).map_err(git_err)?,
            flags: 0,
            flags_extended: 0,
            path: path.as_bytes().to_vec(),
        })
        .map_err(git_err)?;
    let tree = repo
        .find_tree(index.write_tree_to(&repo).map_err(git_err)?)
        .map_err(git_err)?;

    let signature = git2::Signature::now(&author.name, &author.email).map_err(git_err)?;
    let id = repo
        .commit(Some("HEAD"), &signature, &signature, message, &tree, &[&parent])
        .map_err(git_err)?;
    // The name is taken literally. Left to itself, checkout reads it as a
    // pattern, and an entry called `web/*` would overwrite its neighbours.
    let mut only_this_file = CheckoutBuilder::new();
    only_this_file.force().disable_pathspec_match(true).path(path);
    repo.checkout_head(Some(&mut only_this_file)).map_err(git_err)?;
    Ok(id.to_string())
}

/// Pushes the current branch to the branch of the same name, as a
/// fast-forward only.
pub fn push(connection: &Connection, dir: &Path) -> Result<(), PawError> {
    let repo = Repository::open(dir).map_err(git_err)?;
    let (branch, short) = current_branch(&repo)?;
    let tip = repo.refname_to_id(&branch).map_err(git_err)?;

    // No leading `+`: libgit2 refuses, before sending anything, to move the
    // remote branch to a commit that does not contain its current tip.
    let refspec = format!("{branch}:{branch}");
    let mut refused = None;
    connection.with_remote(&repo, |remote| {
        let mut callbacks = RemoteCallbacks::new();
        // A push "succeeds" even when the host rejects the branch update;
        // the rejection only shows up here.
        callbacks.push_update_reference(|_, status| {
            refused = status.map(str::to_owned);
            Ok(())
        });
        let mut options = PushOptions::new();
        options.remote_callbacks(callbacks);
        remote.push(&[&refspec], Some(&mut options))
    })?;

    match refused {
        // The host moved between our look at its branch and our update.
        Some(why) if why.contains("non-fast-forward") || why.contains("fetch first") => {
            Err(PawError::NotFastForward)
        }
        Some(why) => Err(PawError::Git(format!("the host refused the push: {why}"))),
        None => {
            let tracking = format!("refs/remotes/origin/{short}");
            repo.reference(&tracking, tip, true, "push").map_err(git_err)?;
            Ok(())
        }
    }
}

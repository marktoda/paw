//! The building blocks shared by integrate and the unpushed queue.
//!
//! Each paw commit changes one entry (add, edit, delete) or renames one.
//! Replay re-applies that change to the new base tree. Entries are
//! ciphertext, so there is nothing to merge inside a file: either the base
//! still has what the commit started from, or the same entry changed on
//! both sides and both versions are kept.

use std::collections::HashMap;
use std::path::Path;

use git2::{
    Commit, Delta, DiffOptions, FileMode, ObjectType, Oid, Repository, Signature, Sort,
    StatusOptions, Tree,
};

use super::guard::{self, Refused};
use super::{SyncError, git_err};
use crate::PawError;
use crate::path::{unsafe_path, unsafe_path_err};

/// What one paw commit did, as the queue screen names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ChangeKind {
    Added,
    Edited,
    Deleted,
    Renamed,
}

/// A blob and its file mode at some path of a tree.
type Blob = (Oid, i32);

pub(crate) enum Change {
    /// Add or edit.
    Write { path: String, blob: Blob },
    Delete { path: String },
    Rename { from: String, to: String, blob: Blob },
}

impl Change {
    /// The file this change leaves and its blob: the ciphertext it would
    /// send.
    pub(crate) fn ciphertext(&self) -> Option<(&str, Oid)> {
        match self {
            Self::Write { path, blob } | Self::Rename { to: path, blob, .. } => Some((path, blob.0)),
            Self::Delete { .. } => None,
        }
    }
}

/// A local commit that paw made, read back.
pub(crate) struct PawCommit<'r> {
    pub commit: Commit<'r>,
    pub change: Change,
    pub kind: ChangeKind,
    parent_tree: Tree<'r>,
}

impl PawCommit<'_> {
    /// The entry the change is about, for people: no `.gpg`.
    pub fn entry(&self) -> String {
        match &self.change {
            Change::Write { path, .. } | Change::Delete { path } => entry_name(path),
            Change::Rename { to, .. } => entry_name(to),
        }
    }

    /// For a rename, the name it had before.
    pub fn previous(&self) -> Option<String> {
        match &self.change {
            Change::Rename { from, .. } => Some(entry_name(from)),
            _ => None,
        }
    }
}

/// An entry's name as people see it: its path without `.gpg`.
pub(crate) fn entry_name(path: &str) -> String {
    path.strip_suffix(".gpg").unwrap_or(path).to_owned()
}

fn is_regular(mode: u32) -> bool {
    [FileMode::Blob, FileMode::BlobExecutable]
        .map(u32::from)
        .contains(&mode)
}

fn not_paw(commit: &Commit<'_>, reason: &str) -> SyncError {
    SyncError::NotPawHistory { commit: commit.id().to_string(), reason: reason.to_owned() }
}

/// Reads `commit` as a paw commit, or says why it is not one.
pub(crate) fn classify<'r>(repo: &'r Repository, commit: Commit<'r>) -> Result<PawCommit<'r>, SyncError> {
    if commit.parent_count() != 1 {
        return Err(not_paw(&commit, "it does not have exactly one parent"));
    }
    let parent_tree = commit.parent(0).and_then(|p| p.tree()).map_err(git_err)?;
    let tree = commit.tree().map_err(git_err)?;
    let mut options = DiffOptions::new();
    options.include_typechange(true);
    let diff = repo
        .diff_tree_to_tree(Some(&parent_tree), Some(&tree), Some(&mut options))
        .map_err(git_err)?;

    let mut added = Vec::new();
    let mut deleted = Vec::new();
    let mut modified = Vec::new();
    for delta in diff.deltas() {
        let (file, list) = match delta.status() {
            Delta::Added => (delta.new_file(), &mut added),
            Delta::Deleted => (delta.old_file(), &mut deleted),
            Delta::Modified => (delta.new_file(), &mut modified),
            _ => return Err(not_paw(&commit, "it changes the kind of a file")),
        };
        if !is_regular(delta.old_file().mode().into()) && delta.status() != Delta::Added
            || !is_regular(delta.new_file().mode().into()) && delta.status() != Delta::Deleted
        {
            return Err(not_paw(&commit, "it changes something other than a regular file"));
        }
        let Some(path) = file.path().and_then(Path::to_str) else {
            return Err(not_paw(&commit, "it names a file that is not UTF-8"));
        };
        if let Some(why) = unsafe_path(path) {
            return Err(SyncError::from(unsafe_path_err(path, why)));
        }
        list.push((path.to_owned(), (file.id(), u32::from(file.mode()) as i32)));
    }

    let (change, kind) = match (added.as_slice(), deleted.as_slice(), modified.as_slice()) {
        ([(path, blob)], [], []) => (Change::Write { path: path.clone(), blob: *blob }, ChangeKind::Added),
        ([], [], [(path, blob)]) => (Change::Write { path: path.clone(), blob: *blob }, ChangeKind::Edited),
        ([], [(path, _)], []) => (Change::Delete { path: path.clone() }, ChangeKind::Deleted),
        // A rename keeps the blob. A move into a folder with other
        // recipients re-encrypts it (as `pass mv` does), so the blob
        // differs; it is still one entry, moved.
        ([(to, blob)], [(from, old)], [])
            if blob.0 == old.0 || (from.ends_with(".gpg") && to.ends_with(".gpg")) =>
        (
            Change::Rename { from: from.clone(), to: to.clone(), blob: *blob },
            ChangeKind::Renamed,
        ),
        _ => return Err(not_paw(&commit, "it changes more than one entry")),
    };
    Ok(PawCommit { commit, change, kind, parent_tree })
}

/// Commits reachable from `head` and from none of `hide`, oldest first.
pub(crate) fn commits_between(repo: &Repository, head: Oid, hide: &[Oid]) -> Result<Vec<Oid>, SyncError> {
    let mut walk = repo.revwalk().map_err(git_err)?;
    walk.push(head).map_err(git_err)?;
    for id in hide {
        walk.hide(*id).map_err(git_err)?;
    }
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE).map_err(git_err)?;
    walk.map(|id| id.map_err(|e| git_err(e).into())).collect()
}

/// The local commits on `head` that are not in any of `hide`, read as paw
/// commits, oldest first. Any that is not one stops everything.
pub(crate) fn paw_commits<'r>(
    repo: &'r Repository,
    head: Oid,
    hide: &[Oid],
) -> Result<Vec<PawCommit<'r>>, SyncError> {
    commits_between(repo, head, hide)?
        .into_iter()
        .map(|id| classify(repo, repo.find_commit(id).map_err(git_err)?))
        .collect()
}

/// Checks that `path` can be written into `tree`: every folder on the way
/// is a real folder in the tree (or does not exist yet), and the entry
/// itself is a regular file (or does not exist yet).
pub(crate) fn check_tree_path(tree: &Tree<'_>, path: &str) -> Result<(), PawError> {
    if let Some(why) = unsafe_path(path) {
        return Err(unsafe_path_err(path, why));
    }
    for (end, _) in path.match_indices('/') {
        match tree.get_path(Path::new(&path[..end])) {
            Ok(entry) if entry.kind() != Some(ObjectType::Tree) => {
                let why = "one of its folders is a link or a file in the repository";
                return Err(unsafe_path_err(path, why));
            }
            _ => {}
        }
    }
    if let Ok(entry) = tree.get_path(Path::new(path))
        && !is_regular(entry.filemode() as u32)
    {
        return Err(unsafe_path_err(path, "it is a link or a folder in the repository"));
    }
    Ok(())
}

fn at(tree: &Tree<'_>, path: &str) -> Option<Blob> {
    tree.get_path(Path::new(path))
        .ok()
        .map(|entry| (entry.id(), entry.filemode()))
}

/// What happened to entries changed on both sides, for the report.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeptBoth {
    /// The entry, which now holds the remote's version.
    pub entry: String,
    /// Where this device's version was saved.
    pub copy: String,
}

/// What a replay did beyond re-applying commits.
#[derive(Debug, Default)]
pub(crate) struct ReplayNotes {
    pub replayed: u32,
    pub kept_both: Vec<KeptBoth>,
    pub kept_local: Vec<String>,
    pub dropped_deletes: Vec<String>,
}

enum Edit {
    Put(String, Blob),
    Remove(String),
}

/// Builds a new chain of commits on `onto`, one per replayed commit.
/// Nothing outside the object database is touched.
pub(crate) struct Replay<'r> {
    repo: &'r Repository,
    tip: Commit<'r>,
    /// Entries whose local version was saved under another name: later
    /// local commits to the same entry follow it there.
    moved: HashMap<String, String>,
    /// Blobs replaced as they are replayed: `(path, old)` becomes `new`,
    /// in each change and in the tree it started from.
    swaps: HashMap<(String, Oid), Oid>,
    pub notes: ReplayNotes,
}

impl<'r> Replay<'r> {
    pub fn onto(repo: &'r Repository, onto: Oid) -> Result<Self, SyncError> {
        Ok(Self {
            repo,
            tip: repo.find_commit(onto).map_err(git_err)?,
            moved: HashMap::new(),
            swaps: HashMap::new(),
            notes: ReplayNotes::default(),
        })
    }

    pub fn swapping(self, swaps: HashMap<(String, Oid), Oid>) -> Self {
        Self { swaps, ..self }
    }

    fn swapped(&self, path: &str, (id, mode): Blob) -> Blob {
        (self.swaps.get(&(path.to_owned(), id)).copied().unwrap_or(id), mode)
    }

    pub fn tip(&self) -> Oid {
        self.tip.id()
    }

    /// Re-applies `local`'s change on the current tip.
    pub fn apply(&mut self, local: &PawCommit<'_>) -> Result<(), SyncError> {
        let base = self.tip.tree().map_err(git_err)?;
        let mut edits = Vec::new();
        let mut message = String::from_utf8_lossy(local.commit.message_bytes()).into_owned();
        let date = date_of(&local.commit.author().when());

        let mut write = |this: &mut Self, edits: &mut Vec<Edit>, path: &str, blob: Blob| {
            let blob = this.swapped(path, blob);
            let target = this.moved.get(path).cloned().unwrap_or_else(|| path.to_owned());
            let started_from = at(&local.parent_tree, path).map(|was| this.swapped(path, was));
            match at(&base, &target) {
                now if now == started_from || now == Some(blob) => {
                    edits.push(Edit::Put(target, blob));
                }
                // Edited here, deleted there: this device's version stays.
                None => {
                    this.notes.kept_local.push(entry_name(path));
                    edits.push(Edit::Put(target, blob));
                }
                // Changed on both sides: the remote's keeps the name.
                Some(_) => {
                    let copy = free_copy_name(&base, edits, path, &date);
                    message = format!(
                        "Keep this iPhone's version of {} as {}.\n\n{message}",
                        entry_name(path),
                        entry_name(&copy)
                    );
                    this.notes
                        .kept_both
                        .push(KeptBoth { entry: entry_name(path), copy: entry_name(&copy) });
                    this.moved.insert(path.to_owned(), copy.clone());
                    edits.push(Edit::Put(copy, blob));
                }
            }
        };
        let delete = |this: &mut Self, edits: &mut Vec<Edit>, path: &str| {
            let target = this.moved.get(path).cloned().unwrap_or_else(|| path.to_owned());
            match at(&base, &target) {
                // Deleted on both sides.
                None => {}
                now if now == at(&local.parent_tree, path).map(|was| this.swapped(path, was)) => {
                    edits.push(Edit::Remove(target))
                }
                // Changed there, deleted here: the change wins.
                Some(_) => this.notes.dropped_deletes.push(entry_name(path)),
            }
        };

        match &local.change {
            Change::Write { path, blob } => write(self, &mut edits, path, *blob),
            Change::Delete { path } => delete(self, &mut edits, path),
            Change::Rename { from, to, blob } => {
                delete(self, &mut edits, from);
                write(self, &mut edits, to, *blob);
            }
        }

        let mut index = git2::Index::new().map_err(git_err)?;
        index.read_tree(&base).map_err(git_err)?;
        for edit in &edits {
            match edit {
                Edit::Put(path, (id, mode)) => {
                    check_tree_path(&base, path)?;
                    index.add(&index_entry(path, *id, *mode as u32)).map_err(git_err)?;
                }
                Edit::Remove(path) => index.remove_path(Path::new(path)).map_err(git_err)?,
            }
        }
        let tree_id = index.write_tree_to(self.repo).map_err(git_err)?;
        if tree_id == base.id() {
            // Already there, or nothing left to do.
            return Ok(());
        }
        // Every rebuilt commit is pushed, and checked out by other copies:
        // a name of this change may collide with one the remote added.
        let refused = guard::check_tree(self.repo, tree_id).map_err(git_err)?;
        if !refused.is_empty() {
            return Err(Refused(refused).into());
        }
        let tree = self.repo.find_tree(tree_id).map_err(git_err)?;
        let author = local.commit.author();
        let committer = local.commit.committer();
        let committer = Signature::now(
            committer.name().unwrap_or("paw"),
            committer.email().unwrap_or(""),
        )
        .map_err(git_err)?;
        let id = self
            .repo
            .commit(None, &author, &committer, &message, &tree, &[&self.tip])
            .map_err(git_err)?;
        self.tip = self.repo.find_commit(id).map_err(git_err)?;
        self.notes.replayed += 1;
        Ok(())
    }
}

pub(crate) fn index_entry(path: &str, id: Oid, mode: u32) -> git2::IndexEntry {
    git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    }
}

/// `<name> (from this iPhone <date>).gpg`, or with ` 2`, ` 3`... after the
/// bracket if that is taken in `tree` or by a pending edit.
fn free_copy_name(tree: &Tree<'_>, pending: &[Edit], path: &str, date: &str) -> String {
    let (stem, extension) = match path.strip_suffix(".gpg") {
        Some(stem) => (stem, ".gpg"),
        None => (path, ""),
    };
    let taken = |name: &str| {
        tree.get_path(Path::new(name)).is_ok()
            || pending.iter().any(|edit| matches!(edit, Edit::Put(p, _) if p == name))
    };
    let first = format!("{stem} (from this iPhone {date}){extension}");
    if !taken(&first) {
        return first;
    }
    (2u32..)
        .map(|n| format!("{stem} (from this iPhone {date}) {n}{extension}"))
        .find(|name| !taken(name))
        .unwrap_or(first)
}

/// `YYYY-MM-DD` of a commit time, in the time zone it was made in.
fn date_of(time: &git2::Time) -> String {
    let seconds = time.seconds() + i64::from(time.offset_minutes()) * 60;
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Refuses to go on if the working copy differs from `HEAD` in any way
/// other than ignored files. paw commits straight from the tree, so this
/// does not happen in normal use; when it does, nothing is overwritten.
pub(crate) fn ensure_clean(repo: &Repository) -> Result<(), SyncError> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .exclude_submodules(true)
        .disable_pathspec_match(true);
    let statuses = repo.statuses(Some(&mut options)).map_err(git_err)?;
    let paths: Vec<String> = statuses
        .iter()
        .filter(|s| !s.status().is_empty() && !s.status().is_ignored())
        .map(|s| String::from_utf8_lossy(s.path_bytes()).into_owned())
        .collect();
    if paths.is_empty() { Ok(()) } else { Err(SyncError::UncommittedChanges { paths }) }
}

/// Moves `branch` from `from` to `to`, makes the working copy and the index
/// match, and records `seen` as the remote tip last accepted. `to` is
/// checked first ([`guard::check_tree`]); if refused, nothing changes. Either all of
/// that happens, or the branch, the index file and the working copy are put
/// back exactly as they were. The working copy must be clean.
pub(crate) fn move_branch(
    held: &crate::lock::Exclusive,
    repo: &Repository,
    branch: &str,
    from: Oid,
    to: Oid,
    seen: Option<(&str, Oid)>,
    why: &str,
) -> Result<(), SyncError> {
    debug_assert!(repo.workdir().is_none_or(|dir| held.covers(dir)), "move_branch without its store's lock");
    if from != to {
        // Nothing reaches the working copy that has not been checked.
        guard::check_commit(repo, to)?;
        let old_tree = repo.find_commit(from).and_then(|c| c.tree()).map_err(git_err)?;
        let new_tree = repo.find_commit(to).and_then(|c| c.tree()).map_err(git_err)?;
        let index_file = repo.path().join("index");
        let saved_index = std::fs::read(&index_file).ok();
        let restore = || {
            put_back_index(&index_file, saved_index.as_deref());
            let mut back = guard::checkout();
            back.remove_untracked(true);
            let _ = repo.checkout_tree(old_tree.as_object(), Some(&mut back));
            put_back_index(&index_file, saved_index.as_deref());
        };

        let mut forward = guard::checkout();
        if let Err(e) = repo.checkout_tree(new_tree.as_object(), Some(&mut forward)) {
            restore();
            return Err(git_err(e).into());
        }
        if let Err(e) = repo.reference_matching(branch, to, true, from, why) {
            restore();
            return Err(git_err(e).into());
        }
        if let Some((name, tip)) = seen
            && let Err(e) = repo.reference(name, tip, true, why)
        {
            let _ = repo.reference_matching(branch, from, true, to, "undo");
            restore();
            return Err(git_err(e).into());
        }
        return Ok(());
    }
    if let Some((name, tip)) = seen {
        repo.reference(name, tip, true, why).map_err(git_err)?;
    }
    Ok(())
}

fn put_back_index(path: &Path, saved: Option<&[u8]>) {
    let _ = match saved {
        Some(bytes) => std::fs::write(path, bytes),
        None => std::fs::remove_file(path),
    };
}

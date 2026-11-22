//! What is in the store: folders and entry names, read from the directory.
//!
//! Names are all this reads. An entry is a file ending in `.gpg`; anything
//! whose name starts with a dot (`.git`, `.gpg-id`, `.gitattributes`,
//! `.extensions`) is not part of the store's contents. Symbolic links are
//! never followed: a link in a cloned repository can point anywhere on the
//! device, so links are left out and listed in [`StoreTree::skipped`].

use std::collections::HashMap;
use std::path::Path;

use super::read::is_hidden;
use crate::PawError;
use crate::path::unsafe_path;

/// Deeper folders than this are left out. No real store comes close, and a
/// hostile one cannot make the walk run out of stack.
const MAX_DEPTH: usize = 64;

/// One entry: a `.gpg` file, named without its extension.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EntryRef {
    /// The full name `pass` uses, e.g. `web/github.com`.
    pub path: String,
    /// The last part of the path, e.g. `github.com`.
    pub name: String,
    /// The folder it is in, e.g. `web`; empty at the top of the store.
    pub folder: String,
}

/// One folder, as a row in its parent's list.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FolderRef {
    pub path: String,
    pub name: String,
    /// Entries in this folder and in every folder below it.
    pub entry_count: u32,
}

/// The contents of one folder: subfolders first, then entries, each sorted
/// without regard to letter case.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FolderListing {
    /// Empty for the top of the store.
    pub path: String,
    pub folders: Vec<FolderRef>,
    pub entries: Vec<EntryRef>,
}

/// A snapshot of the store's names. Scan again after anything changes the
/// working copy.
pub struct StoreTree {
    listings: HashMap<String, FolderListing>,
    entries: Vec<EntryRef>,
    skipped: Vec<String>,
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() { name.to_owned() } else { format!("{folder}/{name}") }
}

/// Orders names the way a person expects: `apple`, `Banana`, `cherry`.
fn sort_key(name: &str) -> (String, &str) {
    (name.to_lowercase(), name)
}

impl StoreTree {
    /// Walks the checked-out store at `root`.
    pub fn scan(root: &Path) -> Result<Self, PawError> {
        let mut tree = Self { listings: HashMap::new(), entries: Vec::new(), skipped: Vec::new() };
        tree.walk(root, "", 0)
            .map_err(|e| PawError::Store(format!("could not read the store: {e}")))?;
        tree.entries.sort_by(|a, b| sort_key(&a.path).cmp(&sort_key(&b.path)));
        tree.skipped.sort();
        Ok(tree)
    }

    /// Lists `dir` (which is `folder` inside the store) and everything below
    /// it. Returns how many entries that is.
    fn walk(&mut self, dir: &Path, folder: &str, depth: usize) -> std::io::Result<u32> {
        let mut listing =
            FolderListing { path: folder.to_owned(), folders: Vec::new(), entries: Vec::new() };
        let mut count = 0u32;

        for child in std::fs::read_dir(dir)? {
            let child = child?;
            let name = match child.file_name().into_string() {
                Ok(name) => name,
                Err(raw) => {
                    self.skipped.push(join(folder, &raw.to_string_lossy()));
                    continue;
                }
            };
            if is_hidden(&name) {
                continue;
            }
            let path = join(folder, &name);
            // Not followed: for a link this is the link's own type.
            let kind = child.file_type()?;
            // The name check also catches the spellings of `.git` that do
            // not start with a dot on some file systems.
            if kind.is_symlink() || unsafe_path(&name).is_some() {
                self.skipped.push(path);
            } else if kind.is_dir() {
                if depth >= MAX_DEPTH {
                    self.skipped.push(path);
                    continue;
                }
                // One unreadable folder does not hide the rest of the store.
                match self.walk(&child.path(), &path, depth + 1) {
                    // git tracks no empty folders, so neither does the list.
                    Ok(0) => {}
                    Ok(entry_count) => {
                        count += entry_count;
                        listing.folders.push(FolderRef { path, name, entry_count });
                    }
                    Err(_) => self.skipped.push(path),
                }
            } else if kind.is_file()
                && let Some(stem) = name.strip_suffix(".gpg").filter(|stem| !stem.is_empty())
            {
                count += 1;
                listing.entries.push(EntryRef {
                    path: join(folder, stem),
                    name: stem.to_owned(),
                    folder: folder.to_owned(),
                });
            }
        }

        listing.folders.sort_by(|a, b| sort_key(&a.name).cmp(&sort_key(&b.name)));
        listing.entries.sort_by(|a, b| sort_key(&a.name).cmp(&sort_key(&b.name)));
        self.entries.extend(listing.entries.iter().cloned());
        if count > 0 || folder.is_empty() {
            self.listings.insert(folder.to_owned(), listing);
        }
        Ok(count)
    }

    /// The contents of `folder`; the top of the store is the empty string.
    /// `None` if there is no such folder (or it holds no entries).
    pub fn folder(&self, folder: &str) -> Option<&FolderListing> {
        self.listings.get(folder)
    }

    /// Every entry in the store, sorted by path.
    pub fn entries(&self) -> &[EntryRef] {
        &self.entries
    }

    /// Paths that were left out because they are links, are nested too deep,
    /// could not be read, or have a name paw refuses to use.
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }
}

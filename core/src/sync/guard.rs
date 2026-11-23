//! What paw lets libgit2 write to disk.
//!
//! The repository comes from a host that may be hostile, and the phone's
//! file system (APFS) treats names that differ in Unicode normalisation as
//! one name. A tree that holds a link `café -> ..` and, later, a file
//! `café/recipients` (the second name in NFD) would have checkout follow the
//! link and write next to the store, over paw's own state. So, in layers:
//!
//! - [`check_tree`]: every tree is checked before it is checked out or
//!   committed. Only folders and regular files are allowed; names must be
//!   safe ([`crate::path`]); and no two paths may be the same name to a
//!   case- or normalisation-insensitive file system.
//! - [`open`] and [`init`]: every repository paw opens is configured so that
//!   libgit2 itself writes no links (`core.symlinks=false`), refuses the
//!   HFS+ and NTFS spellings of `.git` (`core.protectHFS`,
//!   `core.protectNTFS`), and converts nothing on checkout whatever the
//!   repository's `.gitattributes` say (`info/attributes`, which takes
//!   precedence over them).
//! - [`checkout`]: every checkout writes plain files, never executables.

use std::collections::HashMap;
use std::path::Path;

use git2::build::CheckoutBuilder;
use git2::{ConfigLevel, Oid, Repository};
use unicode_normalization::UnicodeNormalization;

use crate::PawError;
use crate::path::{is_gpg_id_alias, is_hfs_ignored, unsafe_path};

/// A path in a repository that paw will not check out, and why.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RefusedPath {
    /// The path as the repository names it (lossy if it is not UTF-8).
    pub path: String,
    /// Why, in words for people.
    pub reason: String,
}

/// The repository contains something paw will not check out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refused(pub Vec<RefusedPath>);

/// At most this many paths are named in a refusal's message.
const NAMED_IN_MESSAGE: usize = 5;
/// Folders this deep or deeper are refused: no store needs them, and a crafted
/// tree could otherwise make the walk very long.
const MAX_DEPTH: usize = 32;
/// The words the app shows; [`std::fmt::Display`] starts with them.
pub(crate) const REFUSED_PREFIX: &str = "the remote contains something paw will not check out";

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let named: Vec<String> = self
            .0
            .iter()
            .take(NAMED_IN_MESSAGE)
            .map(|refused| format!("{} ({})", refused.path, refused.reason))
            .collect();
        write!(f, "{REFUSED_PREFIX}: {}", named.join("; "))?;
        if self.0.len() > NAMED_IN_MESSAGE {
            write!(f, "; and {} more", self.0.len() - NAMED_IN_MESSAGE)?;
        }
        Ok(())
    }
}

impl From<Refused> for PawError {
    fn from(refused: Refused) -> Self {
        PawError::UnsafePath(refused.to_string())
    }
}

impl From<Refused> for super::SyncError {
    fn from(refused: Refused) -> Self {
        super::SyncError::Failed { message: refused.to_string() }
    }
}

/// How many ways of comparing names [`folded`] gives.
const SCHEMES: usize = 3;

/// The names a case- and normalisation-insensitive file system (APFS,
/// HFS+, a Linux casefold directory) might file `component` under: without
/// the code points HFS+ ignores, decomposed (NFD) before and after, and
/// then (0) fully case-folded (Unicode CaseFolding.txt C+F: `ß` is `ss`,
/// `ﬁ` is `fi`, `ς` is `σ`, the Kelvin sign is `k`), (1) lower-cased and
/// (2) upper-cased. The exact tables APFS uses on a given iOS cannot be
/// checked from here, so two names equal under any one of these are
/// treated as one name.
fn folded(component: &str) -> [String; SCHEMES] {
    // Most names are ASCII: there all three come down to plain ASCII case.
    if component.is_ascii() {
        let lower = component.to_ascii_lowercase();
        return [lower.clone(), lower, component.to_ascii_uppercase()];
    }
    let visible: String = component.chars().filter(|c| !is_hfs_ignored(*c)).nfd().collect();
    let full = unicase::UniCase::unicode(visible.as_str()).to_folded_case();
    [
        full.nfd().collect(),
        visible.chars().flat_map(char::to_lowercase).nfd().collect(),
        visible.chars().flat_map(char::to_uppercase).nfd().collect(),
    ]
}

const TREE: i32 = 0o040000;
const BLOB: i32 = 0o100644;
const BLOB_EXECUTABLE: i32 = 0o100755;
/// Written by very old versions of git; read as a plain file.
const BLOB_GROUP_WRITABLE: i32 = 0o100664;
const LINK: i32 = 0o120000;
const GITLINK: i32 = 0o160000;

/// At most this many files and folders are looked at in one tree. A store
/// of thousands of entries is far below it; a few crafted tree objects
/// that name one subtree many times at each level are far above it.
const MAX_ENTRIES: usize = 100_000;
/// Paths longer than this, in bytes, are refused.
const MAX_PATH_BYTES: usize = 1024;
/// The walk keeps every path (and its folded forms); past this many bytes
/// of paths the tree is refused, so the check itself stays small.
const MAX_TOTAL_PATH_BYTES: usize = 16 << 20;

/// Every path in `tree` paw will not check out, sorted by path; empty if
/// it may be checked out. Refused:
///
/// - links (mode 120000) and submodules (gitlinks, 160000), and any mode
///   other than a folder or a regular file. Executable files are allowed
///   and checked out as plain files (see [`checkout`]): `pass` keeps its
///   extensions, which are executable, in the store;
/// - names that are not UTF-8, contain a slash, or that [`unsafe_path`]
///   refuses (`..`, `.git` in any spelling some file system accepts);
/// - names some file system would take for `.gpg-id` that are not exactly
///   `.gpg-id`, and a `.gpg-id` that is not a regular file;
/// - two paths that are the same name once case (any of the ways in
///   [`folded`]), Unicode normalisation and the code points HFS+ ignores
///   are set aside, whether files or folders (so a file `A` and a folder
///   `a/` collide), and a name that appears twice in one tree;
/// - folders nested 32 deep or more, and paths over 1024 bytes;
/// - the whole tree, when it has more than 100,000 files and folders or
///   more than 16 MiB of paths. The walk stops there, so its time and
///   memory are bounded whatever the repository holds.
pub(crate) fn check_tree(repo: &Repository, tree: Oid) -> Result<Vec<RefusedPath>, git2::Error> {
    let mut refused = Vec::new();
    let mut refuse = |path: &str, reason: &str| {
        refused.push(RefusedPath { path: path.to_owned(), reason: reason.to_owned() });
    };
    // (scheme, folded path) -> the first path filed under it.
    let mut seen: HashMap<(usize, String), String> = HashMap::new();
    let mut folders = vec![(String::new(), [String::new(), String::new(), String::new()], tree, 0usize)];
    let (mut entries, mut path_bytes) = (0usize, 0usize);
    while let Some((prefix, folded_prefix, id, depth)) = folders.pop() {
        let tree = repo.find_tree(id)?;
        for entry in tree.iter() {
            entries += 1;
            path_bytes += prefix.len() + entry.name_bytes().len() + 1;
            if entries > MAX_ENTRIES || path_bytes > MAX_TOTAL_PATH_BYTES {
                refused.clear();
                refused.push(RefusedPath {
                    path: "/".into(),
                    reason: format!(
                        "the repository holds more than {MAX_ENTRIES} files and folders, or more than {} MiB of names",
                        MAX_TOTAL_PATH_BYTES >> 20
                    ),
                });
                return Ok(refused);
            }
            let join = |name: &str| if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") };
            let Ok(name) = std::str::from_utf8(entry.name_bytes()) else {
                refuse(&join(&String::from_utf8_lossy(entry.name_bytes())), "its name is not UTF-8");
                continue;
            };
            let path = join(name);
            let mode = entry.filemode();
            if name.contains('/') {
                refuse(&path, "its name contains a slash");
                continue;
            }
            if path.len() > MAX_PATH_BYTES {
                refuse(&path, "its path is too long");
                continue;
            }
            // The folders above were checked already; the name has no slash.
            if let Some(why) = unsafe_path(name) {
                refuse(&path, why);
                continue;
            }
            if is_gpg_id_alias(name) {
                refuse(&path, "some file systems would take it for .gpg-id");
                continue;
            }
            let names = folded(name);
            let folded_path: [String; SCHEMES] = std::array::from_fn(|scheme| {
                if folded_prefix[scheme].is_empty() {
                    names[scheme].clone()
                } else {
                    format!("{}/{}", folded_prefix[scheme], names[scheme])
                }
            });
            let mut reasons = Vec::new();
            match mode {
                TREE if name == ".gpg-id" => reasons.push("a .gpg-id must be a file".to_owned()),
                TREE if depth + 1 >= MAX_DEPTH => reasons.push("its folders are nested too deeply".to_owned()),
                TREE | BLOB | BLOB_EXECUTABLE | BLOB_GROUP_WRITABLE => {}
                LINK => reasons.push("it is a symbolic link".to_owned()),
                GITLINK => reasons.push("it is a submodule, a link to another repository".to_owned()),
                _ => reasons.push("it is not a regular file".to_owned()),
            }
            let first = (0..SCHEMES).find_map(|scheme| seen.get(&(scheme, folded_path[scheme].clone())));
            match first {
                Some(first) if *first == path => reasons.push("it appears twice in the same folder".to_owned()),
                Some(first) => reasons.push(format!("on an iPhone it is the same name as {first}")),
                None => {
                    for (scheme, key) in folded_path.iter().enumerate() {
                        seen.insert((scheme, key.clone()), path.clone());
                    }
                }
            }
            if !reasons.is_empty() {
                refuse(&path, &reasons.join(", and "));
            } else if mode == TREE {
                folders.push((path, folded_path, entry.id(), depth + 1));
            }
        }
    }
    refused.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(refused)
}

/// [`check_tree`] on the tree of `commit`, as an error.
pub(crate) fn check_commit(repo: &Repository, commit: Oid) -> Result<(), CheckError> {
    let tree = repo.find_commit(commit)?.tree_id();
    let refused = check_tree(repo, tree)?;
    if refused.is_empty() { Ok(()) } else { Err(CheckError::Refused(Refused(refused))) }
}

/// Why [`check_commit`] did not pass.
pub(crate) enum CheckError {
    Refused(Refused),
    Git(git2::Error),
}

impl From<git2::Error> for CheckError {
    fn from(error: git2::Error) -> Self {
        Self::Git(error)
    }
}

impl From<CheckError> for super::SyncError {
    fn from(error: CheckError) -> Self {
        match error {
            CheckError::Refused(refused) => refused.into(),
            CheckError::Git(error) => super::git_err(error).into(),
        }
    }
}

impl From<CheckError> for PawError {
    fn from(error: CheckError) -> Self {
        match error {
            CheckError::Refused(refused) => refused.into(),
            CheckError::Git(error) => super::git_err(error),
        }
    }
}

/// Settings libgit2 reads from the repository's own configuration, and
/// the value paw needs.
const SETTINGS: [(&str, bool); 5] = [
    // A link in a tree is written as a plain file holding its target.
    ("core.symlinks", false),
    // Refuse the HFS+ (ignorable code points) and NTFS (`GIT~1`, `.git.`,
    // `.git::$INDEX_ALLOCATION`) spellings of `.git` on checkout; libgit2
    // only does the HFS+ check by default on macOS.
    ("core.protectHFS", true),
    ("core.protectNTFS", true),
    // No line-ending conversion; entries are ciphertext.
    ("core.autocrlf", false),
    // Executables are checked out as plain files; the difference is not a
    // change.
    ("core.filemode", false),
];

/// Takes precedence over every `.gitattributes` in the repository, so the
/// remote cannot have checkout convert line endings (`text`, `eol`,
/// `crlf`), expand `$Id$` (`ident`) or run a filter in a file.
const ATTRIBUTES: &str = "# Written by paw: the store is ciphertext, so checkout converts nothing.\n\
* -text -crlf -ident -filter\n";

/// Applies paw's settings to `repo` (see the module comment). Writes only
/// what differs.
pub(crate) fn harden(repo: &Repository) -> Result<(), PawError> {
    let config = repo.config().map_err(super::git_err)?;
    let mut local = config.open_level(ConfigLevel::Local).map_err(super::git_err)?;
    for (name, value) in SETTINGS {
        if local.get_bool(name).ok() != Some(value) {
            local.set_bool(name, value).map_err(super::git_err)?;
        }
    }
    let info = repo.path().join("info");
    let attributes = info.join("attributes");
    if std::fs::read_to_string(&attributes).ok().as_deref() != Some(ATTRIBUTES) {
        let io = |e: std::io::Error| PawError::Git(format!("could not write {}: {e}", attributes.display()));
        std::fs::create_dir_all(&info).map_err(io)?;
        std::fs::write(&attributes, ATTRIBUTES).map_err(io)?;
    }
    Ok(())
}

/// Opens the repository at `dir` with paw's settings applied.
pub(crate) fn open(dir: &Path) -> Result<Repository, PawError> {
    let repo = Repository::open(dir).map_err(super::git_err)?;
    harden(&repo)?;
    Ok(repo)
}

/// Creates a repository at `dir` with paw's settings applied.
pub(crate) fn init(dir: &Path) -> Result<Repository, PawError> {
    let repo = Repository::init(dir).map_err(super::git_err)?;
    harden(&repo)?;
    Ok(repo)
}

/// A forced checkout that writes every file as a plain, non-executable file.
pub(crate) fn checkout<'cb>() -> CheckoutBuilder<'cb> {
    let mut builder = CheckoutBuilder::new();
    builder.force().file_perm(0o644);
    builder
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// Whether two names collide under any of [`folded`]'s schemes.
    fn same(a: &str, b: &str) -> bool {
        let (a, b) = (folded(a), folded(b));
        (0..SCHEMES).any(|scheme| a[scheme] == b[scheme])
    }

    /// Pairs a case- or normalisation-insensitive file system may take for
    /// one name; lower-casing alone misses most of them.
    const ONE_NAME: [(&str, &str); 12] = [
        ("Caf\u{e9}", "cafe\u{301}"),
        ("a\u{200c}b", "ab"),
        ("stra\u{df}e", "STRASSE"),
        ("stra\u{df}e", "strasse"),
        ("\u{17f}ecret", "secret"),
        ("\u{212a}ey", "key"),
        ("\u{3c3}\u{3bf}\u{3c6}\u{3bf}\u{3c2}", "\u{3c3}\u{3bf}\u{3c6}\u{3bf}\u{3c3}"),
        ("\u{3a3}", "\u{3c2}"),
        ("\u{131}", "i"),
        ("\u{130}", "i\u{307}"),
        ("\u{fb01}le", "file"),
        ("\u{fb01}le", "FILE"),
    ];

    #[test]
    fn folding_matches_case_and_normalisation() {
        for (a, b) in ONE_NAME {
            assert!(same(a, b), "{a:?} {b:?}");
        }
        assert!(!same("a.", "a"));
        assert!(!same("caf\u{e9}", "cafe"));
    }

    /// A tree of `(path, mode)` built through an index, so any names go.
    fn tree_of(repo: &Repository, files: &[(&str, i32)]) -> Oid {
        let mut index = git2::Index::new().unwrap();
        for (path, mode) in files {
            let blob = repo.blob(b"x").unwrap();
            index.add(&crate::sync::history::index_entry(path, blob, *mode as u32)).unwrap();
        }
        index.write_tree_to(repo).unwrap()
    }

    #[test]
    fn names_that_are_one_name_on_an_iphone_are_refused() {
        let (_dir, repo) = new_repo(true);
        for (a, b) in ONE_NAME {
            // Siblings.
            let refused = check_tree(&repo, tree_of(&repo, &[(a, BLOB), (b, BLOB)])).unwrap();
            assert_eq!(refused.len(), 1, "{a:?} {b:?}: {refused:?}");
            assert!(refused[0].reason.contains("same name as"), "{refused:?}");
            // A link, then a file through it under the other spelling.
            let through = format!("{b}/recipients");
            let refused = check_tree(&repo, tree_of(&repo, &[(a, LINK), (&through, BLOB)])).unwrap();
            assert!(refused.iter().any(|r| r.reason.contains("same name as")), "{a:?} {b:?}: {refused:?}");
            assert!(refused.iter().any(|r| r.path == a && r.reason.contains("symbolic link")), "{refused:?}");
        }
    }

    #[test]
    fn a_tree_that_names_one_subtree_many_times_is_refused_quickly() {
        let (_dir, repo) = new_repo(true);
        // 12 tree objects; 4^11, about four million, paths.
        let blob = repo.blob(b"x").unwrap();
        let mut builder = repo.treebuilder(None).unwrap();
        builder.insert("a.gpg", blob, BLOB).unwrap();
        let mut tree = builder.write().unwrap();
        for _ in 0..11 {
            let mut builder = repo.treebuilder(None).unwrap();
            for name in ["a", "b", "c", "d"] {
                builder.insert(name, tree, TREE).unwrap();
            }
            tree = builder.write().unwrap();
        }
        let started = std::time::Instant::now();
        let refused = check_tree(&repo, tree).unwrap();
        let took = started.elapsed();

        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(refused[0].reason.contains("more than 100000"), "{refused:?}");
        // Stopped at the cap: well under a minute even unoptimised, on a busy machine.
        assert!(took < std::time::Duration::from_secs(60), "{took:?}");
    }

    #[test]
    fn folders_nested_too_deeply_are_refused() {
        let (_dir, repo) = new_repo(true);
        let deep = vec!["d"; 40].join("/") + "/a.gpg";
        let refused = check_tree(&repo, tree_of(&repo, &[(&deep, BLOB)])).unwrap();
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(refused[0].reason.contains("nested too deeply"));
        let fine = vec!["d"; 30].join("/") + "/a.gpg";
        assert!(check_tree(&repo, tree_of(&repo, &[(&fine, BLOB)])).unwrap().is_empty());
    }

    /// A repository at a fresh directory, hardened or as libgit2 makes it.
    fn new_repo(hardened: bool) -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        let repo = if hardened { init(dir.path()).unwrap() } else { Repository::init(dir.path()).unwrap() };
        (dir, repo)
    }

    /// Checks out a tree of `(path, mode, contents)` straight through
    /// libgit2, bypassing [`check_tree`]: what the settings alone do.
    fn checkout_files(repo: &Repository, files: &[(&str, i32, &[u8])]) -> Result<(), git2::Error> {
        let mut index = git2::Index::new()?;
        for (path, mode, contents) in files {
            let blob = repo.blob(contents)?;
            index.add(&crate::sync::history::index_entry(path, blob, *mode as u32))?;
        }
        let tree = repo.find_tree(index.write_tree_to(repo)?)?;
        repo.checkout_tree(tree.as_object(), Some(&mut checkout()))
    }

    #[test]
    fn a_link_is_checked_out_as_a_plain_file() {
        for hardened in [false, true] {
            let (dir, repo) = new_repo(hardened);
            checkout_files(&repo, &[("up", LINK, b"..")]).unwrap();
            let up = std::fs::symlink_metadata(dir.path().join("up")).unwrap();
            // Without the settings libgit2 makes the link (this is Linux).
            assert_eq!(up.is_symlink(), !hardened, "hardened: {hardened}");
        }
        let (dir, repo) = new_repo(true);
        checkout_files(&repo, &[("up", LINK, b"..")]).unwrap();
        assert_eq!(std::fs::read(dir.path().join("up")).unwrap(), b"..");
    }

    #[test]
    fn hfs_and_ntfs_spellings_of_dot_git_are_refused_by_checkout() {
        for name in [".g\u{200c}it/config", ".gi\u{feff}t/config"] {
            let (dir, repo) = new_repo(false);
            checkout_files(&repo, &[(name, BLOB, b"[core]\n")]).unwrap();
            assert!(dir.path().join(name).exists(), "libgit2 alone writes {name:?} on Linux");

            let (dir, repo) = new_repo(true);
            assert!(checkout_files(&repo, &[(name, BLOB, b"[core]\n")]).is_err(), "{name:?}");
            assert!(!dir.path().join(name).exists(), "{name:?}");
        }
        for name in ["GIT~1/config", ".git./config"] {
            let (dir, repo) = new_repo(true);
            assert!(checkout_files(&repo, &[(name, BLOB, b"[core]\n")]).is_err(), "{name:?}");
            assert!(!dir.path().join(name).exists(), "{name:?}");
        }
    }

    #[test]
    fn the_remote_attributes_cannot_change_ciphertext_on_checkout() {
        let ciphertext: &[u8] = b"\x85\x01\x0c\nline\n$Id$\n\r\n";
        for attributes in ["*.gpg text eol=crlf\n", "*.gpg eol=crlf\n", "web/* -text\n*.gpg text eol=crlf\n"] {
            let files: [(&str, i32, &[u8]); 2] =
                [(".gitattributes", BLOB, attributes.as_bytes()), ("web/a.gpg", BLOB, ciphertext)];
            let (dir, repo) = new_repo(false);
            // The attributes are read from the working copy: check them out first.
            checkout_files(&repo, &files[..1]).unwrap();
            checkout_files(&repo, &files).unwrap();
            let unprotected = std::fs::read(dir.path().join("web/a.gpg")).unwrap();
            assert_ne!(unprotected, ciphertext, "libgit2 alone rewrites the entry for {attributes:?}");

            let (dir, repo) = new_repo(true);
            checkout_files(&repo, &files[..1]).unwrap();
            checkout_files(&repo, &files).unwrap();
            assert_eq!(std::fs::read(dir.path().join("web/a.gpg")).unwrap(), ciphertext, "{attributes:?}");
        }
    }

    #[test]
    fn an_executable_is_checked_out_as_a_plain_file_and_is_no_change() {
        let (dir, repo) = new_repo(true);
        let mut index = git2::Index::new().unwrap();
        let blob = repo.blob(b"#!/bin/sh\n").unwrap();
        index.add(&crate::sync::history::index_entry(".extensions/x.bash", blob, BLOB_EXECUTABLE as u32)).unwrap();
        let tree = repo.find_tree(index.write_tree_to(&repo).unwrap()).unwrap();
        let signature = git2::Signature::now("t", "t@example.com").unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "x", &tree, &[]).unwrap();
        repo.checkout_head(Some(&mut checkout())).unwrap();

        let mode = std::fs::metadata(dir.path().join(".extensions/x.bash")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0, "{mode:o}");
        assert!(check_tree(&repo, tree.id()).unwrap().is_empty());
        crate::sync::history::ensure_clean(&repo).unwrap();
    }

    #[test]
    fn opening_an_existing_copy_applies_the_settings() {
        let (dir, repo) = new_repo(false);
        drop(repo);
        let repo = open(dir.path()).unwrap();
        let config = repo.config().unwrap().snapshot().unwrap();
        assert_eq!(config.get_bool("core.symlinks").ok(), Some(false));
        assert_eq!(config.get_bool("core.protectHFS").ok(), Some(true));
        assert_eq!(config.get_bool("core.protectNTFS").ok(), Some(true));
        let attributes = std::fs::read_to_string(dir.path().join(".git/info/attributes")).unwrap();
        assert_eq!(attributes, ATTRIBUTES);
    }
}

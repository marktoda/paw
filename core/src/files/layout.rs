//! What paw keeps for one store, by name: every file and folder in its
//! state directory. On-disk names are part of the format; Swift asks here
//! instead of spelling them.
//!
//! - `setup`: the setup step reached ([`crate::setup`]);
//! - `known_hosts`: host keys the person confirmed ([`crate::sync::hosts`]);
//! - `remote`: the store's address, once a clone succeeded;
//! - `github`: the GitHub repository chosen for the store;
//! - `enrolled-key`: the key's identity, once the test decrypt succeeded;
//! - `recipients`: the pinned keys and accepted `.gpg-id` files;
//! - `site-pairings.json`, `one-time-codes.json`, `one-time-codes-on`:
//!   AutoFill's pairings and the entries with a one-time code;
//! - `key-log.*`: the key log;
//! - `store.lock`: the store's lock;
//! - `repo/`: the store itself.

use std::path::{Path, PathBuf};

pub const REPO: &str = "repo";
pub const SETUP: &str = "setup";
/// The first line of [`SETUP`] once setup has finished.
pub const SETUP_DONE: &str = "step=done";
pub const KNOWN_HOSTS: &str = "known_hosts";
pub const REMOTE: &str = "remote";
pub const GITHUB_LINK: &str = "github";
pub const ENROLLED_KEY: &str = "enrolled-key";
pub const RECIPIENTS: &str = "recipients";
pub const SITE_PAIRINGS: &str = "site-pairings.json";
pub const ONE_TIME_CODES: &str = "one-time-codes.json";
pub const ONE_TIME_CODES_ON: &str = "one-time-codes-on";
pub const KEY_LOG_PREFIX: &str = "key-log.";
pub const STORE_LOCK: &str = "store.lock";

pub fn repo_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(REPO)
}

pub(crate) fn state_dir_of(repo: &Path) -> Option<&Path> {
    if repo.file_name()? != REPO {
        return None;
    }
    repo.parent().filter(|parent| !parent.as_os_str().is_empty())
}

/// Setup finished and the store is here. Reads two files and writes
/// nothing, so the AutoFill extension can ask.
pub fn is_set_up(state_dir: &Path) -> bool {
    let done = super::values::read(&state_dir.join(SETUP))
        .is_ok_and(|found| found.is_some_and(|values| values.first_line() == Some(SETUP_DONE)));
    done && holds_store(&repo_dir(state_dir))
}

/// Whether the checkout at `repo` is a pass store: a `.gpg-id` at its top
/// that is a file, not a link (a link is never checked out). Every "is the
/// store here" asks this.
pub(crate) fn holds_store(repo: &Path) -> bool {
    std::fs::symlink_metadata(repo.join(crate::path::GPG_ID)).is_ok_and(|found| found.is_file())
}

#[uniffi::export]
pub fn store_repo_dir(state_dir: String) -> String {
    repo_dir(Path::new(&state_dir)).to_string_lossy().into_owned()
}

/// Whether setup finished for the store kept in `state_dir` and the store
/// is there. Reads two files; creates nothing.
#[uniffi::export]
pub fn store_is_set_up(state_dir: String) -> bool {
    is_set_up(Path::new(&state_dir))
}

/// Whether the system refuses to open the store's files in `state_dir`, as
/// iOS does for protected files while the iPhone is locked. A file that
/// isn't there is not a refusal.
pub fn files_refused(state_dir: &Path) -> bool {
    [state_dir.join(SETUP), state_dir.join(ENROLLED_KEY), repo_dir(state_dir).join(crate::path::GPG_ID)]
        .iter()
        .any(|path| std::fs::File::open(path).is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied))
}

/// Whether the system refuses to open the store's files in `state_dir`:
/// then a failure to open the store may only mean the iPhone is locked.
#[uniffi::export]
pub fn store_files_refused(state_dir: String) -> bool {
    files_refused(Path::new(&state_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_the_system_wont_open_is_refused_and_a_missing_one_is_not() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        assert!(!files_refused(dir.path()));
        let setup = dir.path().join(SETUP);
        std::fs::write(&setup, SETUP_DONE).unwrap();
        assert!(!files_refused(dir.path()));
        std::fs::set_permissions(&setup, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root ignores the mode, and then there is nothing to check.
        if std::fs::File::open(&setup).is_err() {
            assert!(files_refused(dir.path()));
        }
    }

    #[test]
    fn only_a_repository_named_repo_has_a_state_directory() {
        assert_eq!(state_dir_of(Path::new("/s/repo")), Some(Path::new("/s")));
        assert_eq!(state_dir_of(Path::new("/s/store")), None);
        assert_eq!(state_dir_of(Path::new("repo")), None);
    }
}

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
//! - `git-identity`: who commits made here are attributed to;
//! - `site-pairings.json`, `one-time-codes.json`, `one-time-codes-on`:
//!   AutoFill's pairings and the entries with a one-time code;
//! - `key-log.*`: the key log;
//! - `store.lock`: the store's lock;
//! - `repo/`: the store itself.

use std::path::{Path, PathBuf};

pub(crate) const REPO: &str = "repo";
pub(crate) const SETUP: &str = "setup";
/// The first line of [`SETUP`] once setup has finished.
pub(crate) const SETUP_DONE: &str = "step=done";
pub(crate) const KNOWN_HOSTS: &str = "known_hosts";
pub(crate) const REMOTE: &str = "remote";
pub(crate) const GITHUB_LINK: &str = "github";
pub(crate) const ENROLLED_KEY: &str = "enrolled-key";
pub(crate) const RECIPIENTS: &str = "recipients";
pub(crate) const GIT_IDENTITY: &str = "git-identity";
pub(crate) const SITE_PAIRINGS: &str = "site-pairings.json";
pub(crate) const ONE_TIME_CODES: &str = "one-time-codes.json";
pub(crate) const ONE_TIME_CODES_ON: &str = "one-time-codes-on";
pub(crate) const KEY_LOG_PREFIX: &str = "key-log.";
pub(crate) const STORE_LOCK: &str = "store.lock";

/// Where the store of `state_dir` is checked out.
pub fn repo_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(REPO)
}

/// The state directory of a repository checked out at `<state>/repo`;
/// `None` for a repository anywhere else.
pub(crate) fn state_dir_of(repo: &Path) -> Option<&Path> {
    if repo.file_name()? != REPO {
        return None;
    }
    repo.parent().filter(|parent| !parent.as_os_str().is_empty())
}

/// Setup finished and the store is here. Reads two files and writes
/// nothing, so the AutoFill extension can ask.
pub fn is_set_up(state_dir: &Path) -> bool {
    let done = std::fs::read_to_string(state_dir.join(SETUP))
        .is_ok_and(|text| text.lines().next() == Some(SETUP_DONE));
    done && repo_dir(state_dir).join(crate::path::GPG_ID).is_file()
}

/// Where the store kept in `state_dir` is checked out.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_repository_named_repo_has_a_state_directory() {
        assert_eq!(state_dir_of(Path::new("/s/repo")), Some(Path::new("/s")));
        assert_eq!(state_dir_of(Path::new("/s/store")), None);
        assert_eq!(state_dir_of(Path::new("repo")), None);
    }
}

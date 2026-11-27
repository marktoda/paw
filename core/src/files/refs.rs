//! The refs paw keeps in a store's repository, by name. `short` is a
//! branch's name without `refs/heads/`.

use std::path::{Path, PathBuf};

const HEADS: &str = "refs/heads/";
/// Where a fetch puts the remote's branches until they are published.
const FETCHED: &str = "refs/paw/fetch";

pub(crate) fn short(branch: &str) -> Option<&str> {
    branch.strip_prefix(HEADS)
}

pub(crate) fn remote_tracking(short: &str) -> String {
    format!("refs/remotes/origin/{short}")
}

/// The tip of the remote branch as paw last accepted it. A remote tip that
/// does not descend from it means the history was replaced.
pub(crate) fn last_seen(short: &str) -> String {
    format!("refs/paw/last-seen/{short}")
}

pub(crate) fn fetch_refspec() -> String {
    format!("+{HEADS}*:{FETCHED}/*")
}

/// The branch a ref in the private fetch namespace is the remote's copy of.
pub(crate) fn fetched_branch(name: &str) -> Option<&str> {
    name.strip_prefix(FETCHED)?.strip_prefix('/')
}

/// The folders under `.git` that hold the private fetch refs and their
/// logs: written under the fetch lock, not the store lock.
pub(crate) fn fetched_dirs(git: &Path) -> [PathBuf; 2] {
    [git.join(FETCHED), git.join("logs").join(FETCHED)]
}

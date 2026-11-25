//! The refs paw keeps in a store's repository, by name. `short` is a
//! branch's name without `refs/heads/`.

use std::path::{Path, PathBuf};

const HEADS: &str = "refs/heads/";
/// Where a fetch puts the remote's branches until they are published.
const FETCHED: &str = "refs/paw/fetch";
const PUSHING: &str = "refs/paw/pushing/";

/// The branch `short`'s name without `refs/heads/`; `None` for a ref that
/// is not a branch.
pub(crate) fn short(branch: &str) -> Option<&str> {
    branch.strip_prefix(HEADS)
}

/// The remote's branch `short` as last fetched: `refs/remotes/origin/<short>`.
pub(crate) fn remote_tracking(short: &str) -> String {
    format!("refs/remotes/origin/{short}")
}

/// The tip of the remote branch as paw last accepted it. A remote tip that
/// does not descend from it means the history was replaced.
pub(crate) fn last_seen(short: &str) -> String {
    format!("refs/paw/last-seen/{short}")
}

/// The commit a push of `short` has on the network, while it does.
pub(crate) fn pushing(short: &str) -> String {
    format!("{PUSHING}{short}")
}

pub(crate) fn is_pushing(name: &str) -> bool {
    name.starts_with(PUSHING)
}

/// Every branch of the remote, mirrored into the private fetch namespace.
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

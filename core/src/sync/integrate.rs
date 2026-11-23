//! Bringing what was fetched into the local branch, without a merge.
//!
//! The local branch is always the remote's tip plus paw's own commits, one
//! per change. After a fetch:
//! - nothing local: the branch and the working copy fast-forward;
//! - local commits and an unchanged remote: nothing to do;
//! - both moved: each local commit is replayed, oldest first, on the
//!   remote's tip (see [`super::history::Replay`]);
//! - the remote's history was replaced: nothing happens until the person
//!   decides ([`accept_replaced_remote`]).
//!
//! All of it is built as new objects first; the branch, index and working
//! copy move last, together, or not at all.

use std::path::Path;

use git2::{Oid, Repository};

use super::history::{
    KeptBoth, Replay, ReplayNotes, commits_between, ensure_clean, last_seen_ref, move_branch,
    paw_commits,
};
use super::guard::check_commit;
use super::{SyncError, current_branch, git_err};
use crate::store::gpg_id::parse_gpg_id;

/// A `.gpg-id` that differs between the remote tip paw saw last and the
/// one it just fetched. paw always says when recipients change.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RecipientsChange {
    /// The file, e.g. `.gpg-id` or `work/.gpg-id`.
    pub path: String,
    /// The key lines before, without comments; empty if it was added.
    pub before: Vec<String>,
    /// The key lines now; empty if it was removed.
    pub after: Vec<String>,
}

/// What an integrate did. Entry names are for showing only.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct IntegrateReport {
    /// Remote commits the local branch now includes.
    pub fast_forwarded: u32,
    /// Local commits rebuilt on top of the remote's tip.
    pub replayed: u32,
    /// Entries changed on both sides: the remote's version keeps the name.
    pub kept_both: Vec<KeptBoth>,
    /// Entries deleted on the remote but changed here: kept.
    pub kept_local: Vec<String>,
    /// Entries deleted here but changed on the remote: the delete was dropped.
    pub dropped_deletes: Vec<String>,
    pub recipients_changed: Vec<RecipientsChange>,
}

impl IntegrateReport {
    fn note(&mut self, notes: ReplayNotes) {
        self.replayed += notes.replayed;
        self.kept_both.extend(notes.kept_both);
        self.kept_local.extend(notes.kept_local);
        self.dropped_deletes.extend(notes.dropped_deletes);
    }

    /// Adds a later integrate's report to this one.
    pub fn absorb(&mut self, later: IntegrateReport) {
        self.fast_forwarded += later.fast_forwarded;
        self.replayed += later.replayed;
        self.kept_both.extend(later.kept_both);
        self.kept_local.extend(later.kept_local);
        self.dropped_deletes.extend(later.dropped_deletes);
        self.recipients_changed.extend(later.recipients_changed);
    }
}

/// The remote's history no longer contains the tip paw saw last.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Replaced {
    pub last_seen: String,
    pub remote: String,
    /// Local changes that would be replayed onto the new history.
    pub unpushed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Integration {
    Integrated(IntegrateReport),
    /// Nothing was changed; the person has to choose.
    RemoteReplaced(Replaced),
}

struct Tips {
    branch: String,
    seen_ref: String,
    head: Oid,
    remote: Option<Oid>,
    seen: Option<Oid>,
}

fn tips(repo: &Repository) -> Result<Tips, SyncError> {
    let (branch, short) = current_branch(repo)?;
    let head = repo.refname_to_id(&branch).map_err(git_err)?;
    let remote = repo.refname_to_id(&format!("refs/remotes/origin/{short}")).ok();
    let seen_ref = last_seen_ref(&short);
    let seen = repo.refname_to_id(&seen_ref).ok();
    Ok(Tips { branch, seen_ref, head, remote, seen })
}

/// Integrates `origin/<branch>` into the current branch of the store at
/// `dir`. Run after [`super::fetch`].
///
/// If the remote's tip, or a commit rebuilt on it, holds something paw
/// will not check out ([`super::refused_remote`]), this fails with
/// [`SyncError::Failed`] naming the paths, and the branch, the working
/// copy and the unpushed changes stay as they were.
pub fn integrate(dir: &Path) -> Result<Integration, SyncError> {
    let repo = super::guard::open(dir)?;
    let tips = tips(&repo)?;
    let Some(remote) = tips.remote else {
        return Ok(Integration::Integrated(IntegrateReport::default()));
    };
    // What was fetched is checked before anything else is done with it.
    check_commit(&repo, remote)?;
    // A copy cloned before paw kept this ref: what it builds on counts as seen.
    let seen = match tips.seen {
        Some(seen) => seen,
        None => repo.merge_base(tips.head, remote).unwrap_or(remote),
    };
    if seen != remote && !repo.graph_descendant_of(remote, seen).map_err(git_err)? {
        let unpushed = commits_between(&repo, tips.head, &[seen])?.len() as u32;
        return Ok(Integration::RemoteReplaced(Replaced {
            last_seen: seen.to_string(),
            remote: remote.to_string(),
            unpushed,
        }));
    }

    let local = paw_commits(&repo, tips.head, &[remote])?;
    ensure_clean(&repo)?;
    let mut report = IntegrateReport {
        recipients_changed: recipients_changes(&repo, seen, remote)?,
        ..Default::default()
    };
    let remote_is_in_head =
        tips.head == remote || repo.graph_descendant_of(tips.head, remote).map_err(git_err)?;
    let new_head = if remote_is_in_head {
        tips.head
    } else {
        report.fast_forwarded = commits_between(&repo, remote, &[tips.head])?.len() as u32;
        let mut replay = Replay::onto(&repo, remote)?;
        for commit in &local {
            replay.apply(commit)?;
        }
        let tip = replay.tip();
        report.note(replay.notes);
        tip
    };
    move_branch(&repo, &tips.branch, tips.head, new_head, Some((&tips.seen_ref, remote)), "paw: integrate")?;
    Ok(Integration::Integrated(report))
}

/// After [`Integration::RemoteReplaced`], takes the remote's new history:
/// with this device's unpushed changes replayed onto it (`keep_changes`),
/// or without them.
pub fn accept_replaced_remote(dir: &Path, keep_changes: bool) -> Result<IntegrateReport, SyncError> {
    let repo = super::guard::open(dir)?;
    let tips = tips(&repo)?;
    let Some(remote) = tips.remote else {
        return Ok(IntegrateReport::default());
    };
    check_commit(&repo, remote)?;
    let seen = tips.seen.unwrap_or(remote);
    let local = if keep_changes { paw_commits(&repo, tips.head, &[seen, remote])? } else { Vec::new() };
    ensure_clean(&repo)?;
    let mut report = IntegrateReport {
        recipients_changed: recipients_changes(&repo, seen, remote)?,
        ..Default::default()
    };
    let mut replay = Replay::onto(&repo, remote)?;
    for commit in &local {
        replay.apply(commit)?;
    }
    let tip = replay.tip();
    report.note(replay.notes);
    move_branch(&repo, &tips.branch, tips.head, tip, Some((&tips.seen_ref, remote)), "paw: take replaced remote")?;
    Ok(report)
}

/// Every `.gpg-id` that differs between `before` and `after`.
fn recipients_changes(repo: &Repository, before: Oid, after: Oid) -> Result<Vec<RecipientsChange>, SyncError> {
    if before == after {
        return Ok(Vec::new());
    }
    let old = repo.find_commit(before).and_then(|c| c.tree()).map_err(git_err)?;
    let new = repo.find_commit(after).and_then(|c| c.tree()).map_err(git_err)?;
    let diff = repo.diff_tree_to_tree(Some(&old), Some(&new), None).map_err(git_err)?;
    let lines = |id: Oid| -> Vec<String> {
        if id.is_zero() {
            return Vec::new();
        }
        repo.find_blob(id)
            .map(|blob| parse_gpg_id(&String::from_utf8_lossy(blob.content())))
            .unwrap_or_default()
    };
    let mut changes = Vec::new();
    for delta in diff.deltas() {
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .and_then(Path::to_str)
            .unwrap_or_default();
        if path.rsplit('/').next() != Some(".gpg-id") {
            continue;
        }
        changes.push(RecipientsChange {
            path: path.to_owned(),
            before: lines(delta.old_file().id()),
            after: lines(delta.new_file().id()),
        });
    }
    Ok(changes)
}

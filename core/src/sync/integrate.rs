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

use super::guard::{self, check_commit};
use super::history::{KeptBoth, PawCommit, Replay, ReplayNotes, commits_between, ensure_clean, move_branch};
use super::queue::waiting;
use super::{OpenStore, SyncError, current_branch, git_err};
use crate::fault::Fault;
use crate::files::layout::StoreDirs;
use crate::files::refs;
use crate::remote::RefusedPath;
use crate::store::gpg_id::shown_gpg_id_lines;
use crate::lock::{Exclusive, Shared};

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

/// What an integrate did. Entry names are for showing only, never for a
/// log.
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
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
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
        for entry in later.added {
            if take(&mut self.removed, &entry) {
                self.changed.push(entry);
            } else {
                self.added.push(entry);
            }
        }
        for entry in later.changed {
            if !self.added.contains(&entry) && !self.changed.contains(&entry) {
                self.changed.push(entry);
            }
        }
        for entry in later.removed {
            if !take(&mut self.added, &entry) {
                take(&mut self.changed, &entry);
                self.removed.push(entry);
            }
        }
    }

    fn note_entries(&mut self, repo: &Repository, before: Oid, after: Oid) -> Result<(), SyncError> {
        if before == after {
            return Ok(());
        }
        let tree = |id: Oid| repo.find_commit(id).and_then(|c| c.tree()).map_err(git_err);
        let diff = repo.diff_tree_to_tree(Some(&tree(before)?), Some(&tree(after)?), None).map_err(git_err)?;
        for delta in diff.deltas() {
            let path = |file: git2::DiffFile<'_>| file.path().and_then(Path::to_str).map(str::to_owned);
            let list = match delta.status() {
                git2::Delta::Added => &mut self.added,
                git2::Delta::Deleted => &mut self.removed,
                _ => &mut self.changed,
            };
            let file = if delta.status() == git2::Delta::Deleted { delta.old_file() } else { delta.new_file() };
            if let Some(entry) = path(file).as_deref().and_then(crate::path::listed_entry) {
                list.push(entry.to_owned());
            }
        }
        Ok(())
    }
}

/// Removes `entry` from `list`; whether it was there.
fn take(list: &mut Vec<String>, entry: &str) -> bool {
    let before = list.len();
    list.retain(|e| e != entry);
    list.len() != before
}

/// The remote's history no longer contains the tip paw saw last.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Replaced {
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
    let remote = repo.refname_to_id(&refs::remote_tracking(&short)).ok();
    let seen_ref = refs::last_seen(&short);
    let seen = repo.refname_to_id(&seen_ref).ok();
    Ok(Tips { branch, seen_ref, head, remote, seen })
}

struct Fetched {
    remote: Oid,
    /// The remote's tip paw saw last.
    seen: Oid,
    /// The remote's history no longer contains `seen`.
    replaced: bool,
}

fn fetched(repo: &Repository, tips: &Tips) -> Result<Option<Fetched>, SyncError> {
    let Some(remote) = tips.remote else {
        return Ok(None);
    };
    // Every clone records it with the branch. A copy cloned before paw
    // did is set up again rather than guessed at.
    let Some(seen) = tips.seen else {
        return Err(Fault::local("this copy has no record of what it last fetched; start setup again").into());
    };
    let replaced = seen != remote && !repo.graph_descendant_of(remote, seen).map_err(git_err)?;
    Ok(Some(Fetched { remote, seen, replaced }))
}

/// What integrating `origin/<branch>` into the current branch would do,
/// worked out without changing anything ([`plan`]); [`apply`] does it.
pub(crate) struct Plan {
    tips: Tips,
    fetched: Option<Fetched>,
    /// What the fetched tip holds that paw will not check out.
    pub(crate) refused: Vec<RefusedPath>,
    /// The `.gpg-id` changes integrating would report; none if the
    /// remote's history was replaced (that is its own decision).
    pub(crate) recipients_changes: Vec<RecipientsChange>,
    /// The remote's history was replaced: integrating changes nothing.
    pub(crate) replaced: Option<Replaced>,
}

pub(crate) fn plan(_reading: &Shared, store: &OpenStore) -> Result<Plan, SyncError> {
    let repo = &store.repo;
    let tips = tips(repo)?;
    let mut plan = Plan { fetched: fetched(repo, &tips)?, tips, refused: Vec::new(), recipients_changes: Vec::new(), replaced: None };
    let Some(Fetched { remote, seen, replaced }) = plan.fetched else {
        return Ok(plan);
    };
    plan.refused = guard::check_tree(repo, repo.find_commit(remote).map_err(git_err)?.tree_id()).map_err(git_err)?;
    if replaced {
        // Every local commit is counted, paw's or not: this decision is
        // also the way out of a history paw would refuse to replay.
        let unpushed = commits_between(repo, plan.tips.head, &[seen])?.len() as u32;
        plan.replaced = Some(Replaced { remote: remote.to_string(), unpushed });
    } else {
        plan.recipients_changes = recipients_changes(repo, seen, remote)?;
    }
    Ok(plan)
}

/// Integrates as `plan` says.
///
/// If the remote's tip, or a commit rebuilt on it, holds something paw
/// will not check out ([`super::refused_remote`]), this fails with
/// [`crate::remote::RemoteFailure::RefusedContent`] naming the paths, and
/// the branch, the working copy and the unpushed changes stay as they were.
pub(crate) fn apply(held: &Exclusive, store: &OpenStore, plan: Plan) -> Result<Integration, SyncError> {
    let repo = &store.repo;
    let Plan { tips, fetched, refused, recipients_changes, replaced } = plan;
    let Some(Fetched { remote, .. }) = fetched else {
        return Ok(Integration::Integrated(IntegrateReport::default()));
    };
    if !refused.is_empty() {
        return Err(guard::Refused(refused).into());
    }
    if let Some(replaced) = replaced {
        return Ok(Integration::RemoteReplaced(replaced));
    }

    let local = waiting(held, store)?.commits;
    let mut report = IntegrateReport { recipients_changed: recipients_changes, ..Default::default() };
    let remote_is_in_head =
        tips.head == remote || repo.graph_descendant_of(tips.head, remote).map_err(git_err)?;
    let (onto, local) = if remote_is_in_head {
        (tips.head, &[][..])
    } else {
        report.fast_forwarded = commits_between(repo, remote, &[tips.head])?.len() as u32;
        (remote, &local[..])
    };
    Ok(Integration::Integrated(land(held, store, &tips, onto, remote, local, report, "paw: integrate")?))
}

/// Rebuilds `local` on `onto`, then moves the branch, index and working
/// copy there together, and last-seen to `remote`. Returns `report` with
/// what that did.
#[allow(clippy::too_many_arguments)]
fn land(
    held: &Exclusive,
    store: &OpenStore,
    tips: &Tips,
    onto: Oid,
    remote: Oid,
    local: &[PawCommit<'_>],
    mut report: IntegrateReport,
    why: &str,
) -> Result<IntegrateReport, SyncError> {
    let repo = &store.repo;
    ensure_clean(repo)?;
    let mut replay = Replay::onto(repo, onto)?;
    for commit in local {
        replay.apply(commit)?;
    }
    let tip = replay.tip();
    report.note(replay.notes);
    report.note_entries(repo, tips.head, tip)?;
    move_branch(held, store, &tips.branch, tips.head, tip, Some((&tips.seen_ref, remote)), why)?;
    Ok(report)
}

/// After [`Integration::RemoteReplaced`], takes the remote's new history:
/// with this device's unpushed changes replayed onto it (`keep_changes`),
/// or without them.
pub fn accept_replaced_remote(dirs: &StoreDirs, keep_changes: bool) -> Result<IntegrateReport, SyncError> {
    let held = dirs.exclusive()?;
    let store = OpenStore::open(dirs)?;
    let repo = &store.repo;
    let tips = tips(repo)?;
    let Some(Fetched { remote, seen, .. }) = fetched(repo, &tips)? else {
        return Ok(IntegrateReport::default());
    };
    check_commit(repo, remote)?;
    let local = if keep_changes { waiting(&held, &store)?.commits } else { Vec::new() };
    let report = IntegrateReport { recipients_changed: recipients_changes(repo, seen, remote)?, ..Default::default() };
    land(&held, &store, &tips, remote, remote, &local, report, "paw: take replaced remote")
}

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
        repo.find_blob(id).map(|blob| shown_gpg_id_lines(blob.content())).unwrap_or_default()
    };
    let mut changes = Vec::new();
    for delta in diff.deltas() {
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .and_then(Path::to_str)
            .unwrap_or_default();
        if path.rsplit('/').next() != Some(crate::path::GPG_ID) {
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

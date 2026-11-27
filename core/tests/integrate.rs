//! Keeping the phone's copy in step with the remote: integrate after a
//! fetch, a whole sync on one login, and the unpushed queue. Two clones of
//! one bare repository stand in for the phone and the desktop; the phone
//! talks to the in-process SSH server (see `support`), the desktop uses the
//! git command line on the bare repository directly.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use paw_core::sim::remote::steps;
use paw_core::sim::SimulatedKey;
use paw_core::remote::RemoteFailure;
use paw_core::sync::operation::SyncState;
use paw_core::sync::queue;
use paw_core::sync::{
    self, ChangeKind, Cloned, Connection, HostKeyPin, IntegrateReport, Integration, KeptBoth,
    RecipientsChange, SyncError,
};
use paw_core::files::layout::StoreDirs;
use paw_core::write::{Author, WriteStore};
use key_common::{PIN, signer, sync_failure};
use support::{GitServer, git, move_entry};

struct World {
    card: Arc<SimulatedKey>,
    server: GitServer,
    desktop: tempfile::TempDir,
    /// The phone's state directory, whose `repo/` is the phone's clone.
    state: tempfile::TempDir,
    store: StoreDirs,
}

/// A card that signs without a touch, so the tests need not touch it.
fn card() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), false, 50)
}

fn author() -> Author {
    Author { name: "iPhone".into(), email: "iphone@example.com".into() }
}

/// A server with `web/example.com.gpg` and `web/other.gpg`, a phone clone
/// and a desktop clone of it.
fn world() -> World {
    let card = card();
    let server = GitServer::start(&[card.ssh_public_key()]);
    let desktop = server.desktop();
    let state = tempfile::tempdir().unwrap();
    let store = StoreDirs::new(state.path());
    let w = World { card, server, desktop, state, store };
    desktop_write(&w, "web/example.com.gpg", "v1 from the desktop");
    desktop_write(&w, "web/other.gpg", "other");
    desktop_write(&w, ".gpg-id", "AAAA1111\n");
    let connection = Connection::open(&w.server.remote(), &signer(&w.card)).unwrap();
    assert_eq!(steps::clone(&connection, &w.server.remote(), &w.store).unwrap(), Cloned::Store);
    w
}

fn phone(w: &World) -> &Path {
    w.store.repo_dir()
}

fn phone_write(w: &World, path: &str, contents: &str) -> String {
    let message = format!("Edit password for {path} using paw.");
    steps::commit(&w.store, path, contents.as_bytes(), &message, &author()).unwrap()
}

/// paw's delete, through its write API.
fn phone_delete(w: &World, entry: &str) {
    WriteStore::open(w.state.path()).delete(entry).unwrap();
}

/// paw's rename, through its write API: the same encrypted file, renamed.
fn phone_rename(w: &World, from: &str, to: &str) {
    move_entry(&WriteStore::open(w.state.path()), from, to, None).unwrap();
}

/// A commit made with the git command line, as another tool would.
fn phone_git(w: &World, args: &[&str], message: &str) {
    git(phone(w), args);
    git(phone(w), &["-c", "user.name=iPhone", "-c", "user.email=iphone@example.com", "commit", "--quiet", "-m", message]);
}

fn desktop_git(w: &World, args: &[&str]) -> String {
    git(w.desktop.path(), args)
}

fn desktop_push(w: &World, message: &str) -> String {
    desktop_git(w, &["add", "-A"]);
    desktop_git(w, &["commit", "--quiet", "-m", message]);
    desktop_git(w, &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    w.server.tip()
}

fn desktop_write(w: &World, path: &str, contents: &str) -> String {
    let file = w.desktop.path().join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, contents).unwrap();
    desktop_push(w, &format!("Edit {path} on the desktop."))
}

fn desktop_rm(w: &World, path: &str) -> String {
    desktop_git(w, &["rm", "--quiet", path]);
    desktop_push(w, &format!("Remove {path} on the desktop."))
}

/// Fetch, then integrate.
fn pull(w: &World) -> Result<Integration, SyncError> {
    let connection = Connection::open(&w.server.remote(), &signer(&w.card)).unwrap();
    steps::fetch(&connection, &w.store).unwrap();
    steps::integrate(&w.store)
}

fn integrated(w: &World) -> IntegrateReport {
    match pull(w).unwrap() {
        Integration::Integrated(report) => report,
        other => panic!("expected an integrate, got {other:?}"),
    }
}

/// Every file in the working copy but `.git`, with its bytes.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for child in std::fs::read_dir(&folder).unwrap() {
            let path = child.unwrap().path();
            let name = path.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                found.insert(name, std::fs::read(&path).unwrap());
            }
        }
    }
    found
}

fn read(w: &World, path: &str) -> String {
    std::fs::read_to_string(phone(w).join(path)).unwrap()
}

/// Everything a failed operation must leave alone, byte for byte: refs,
/// `HEAD`, the index file and the working copy.
#[derive(Debug, PartialEq)]
struct Snapshot {
    refs: String,
    head: Vec<u8>,
    index: Vec<u8>,
    files: BTreeMap<String, Vec<u8>>,
}

fn snapshot(dir: &Path) -> Snapshot {
    Snapshot {
        refs: git(dir, &["for-each-ref", "--format=%(refname) %(objectname)"]),
        head: std::fs::read(dir.join(".git/HEAD")).unwrap(),
        index: std::fs::read(dir.join(".git/index")).unwrap(),
        files: files(dir),
    }
}

fn head(w: &World) -> String {
    git(phone(w), &["rev-parse", "HEAD"])
}

fn clean(w: &World) {
    assert_eq!(git(phone(w), &["status", "--porcelain"]), "");
}

fn today_of(w: &World, rev: &str) -> String {
    git(phone(w), &["log", "-1", "--date=format:%Y-%m-%d", "--format=%ad", rev])
}

// --- integrate -------------------------------------------------------------

#[test]
fn nothing_local_fast_forwards_the_branch_and_the_working_copy() {
    let w = world();
    desktop_write(&w, "web/example.com.gpg", "v2 from the desktop");
    let tip = desktop_write(&w, "bank/new.gpg", "new");

    let report = integrated(&w);

    assert_eq!(report.fast_forwarded, 2);
    assert_eq!(report.replayed, 0);
    assert_eq!(head(&w), tip);
    assert_eq!(read(&w, "web/example.com.gpg"), "v2 from the desktop");
    assert_eq!(read(&w, "bank/new.gpg"), "new");
    clean(&w);
}

#[test]
fn the_report_names_the_entries_the_remote_added_changed_and_removed() {
    let w = world();
    phone_write(&w, "web/mine.gpg", "mine");
    desktop_write(&w, "bank/new.gpg", "new");
    desktop_write(&w, "web/example.com.gpg", "v2 from the desktop");
    desktop_write(&w, "notes.txt", "not an entry");
    desktop_write(&w, ".hidden/secret.gpg", "hidden as the listing hides it");
    desktop_write(&w, "web/.old.gpg", "hidden too");
    desktop_rm(&w, "web/other.gpg");

    let report = integrated(&w);

    assert_eq!(report.added, ["bank/new"]);
    assert_eq!(report.changed, ["web/example.com"]);
    assert_eq!(report.removed, ["web/other"]);
    clean(&w);
}

#[test]
fn local_commits_on_an_unchanged_remote_are_left_alone() {
    let w = world();
    let mine = phone_write(&w, "web/mine.gpg", "mine");

    let report = integrated(&w);

    assert_eq!(report, IntegrateReport::default());
    assert_eq!(head(&w), mine);
    clean(&w);
}

#[test]
fn a_local_add_is_replayed_on_top_of_the_remote() {
    let w = world();
    phone_write(&w, "web/mine.gpg", "mine");
    let remote = desktop_write(&w, "web/theirs.gpg", "theirs");

    let report = integrated(&w);

    assert_eq!((report.fast_forwarded, report.replayed), (1, 1));
    assert_eq!(git(phone(&w), &["rev-parse", "HEAD~1"]), remote);
    assert_eq!(git(phone(&w), &["show", "--format=", "--name-status", "HEAD"]), "A\tweb/mine.gpg");
    assert_eq!(read(&w, "web/mine.gpg"), "mine");
    assert_eq!(read(&w, "web/theirs.gpg"), "theirs");
    clean(&w);
}

#[test]
fn a_replayed_commit_keeps_its_message_author_and_author_time() {
    let w = world();
    let original = phone_write(&w, "web/mine.gpg", "mine");
    let before = git(phone(&w), &["log", "-1", "--format=%B|%an|%ae|%at", &original]);
    desktop_write(&w, "web/theirs.gpg", "theirs");

    integrated(&w);

    assert_ne!(head(&w), original);
    assert_eq!(git(phone(&w), &["log", "-1", "--format=%B|%an|%ae|%at", "HEAD"]), before);
}

#[test]
fn a_local_edit_is_replayed() {
    let w = world();
    phone_write(&w, "web/example.com.gpg", "v2 from the phone");
    desktop_write(&w, "web/theirs.gpg", "theirs");

    let report = integrated(&w);

    assert_eq!(report.replayed, 1);
    assert!(report.kept_both.is_empty());
    assert_eq!(read(&w, "web/example.com.gpg"), "v2 from the phone");
    assert_eq!(read(&w, "web/theirs.gpg"), "theirs");
    clean(&w);
}

#[test]
fn a_local_delete_is_replayed() {
    let w = world();
    phone_delete(&w, "web/other");
    desktop_write(&w, "web/theirs.gpg", "theirs");

    let report = integrated(&w);

    assert_eq!(report.replayed, 1);
    assert!(!phone(&w).join("web/other.gpg").exists());
    assert_eq!(git(phone(&w), &["show", "--format=", "--name-status", "HEAD"]), "D\tweb/other.gpg");
    clean(&w);
}

#[test]
fn a_local_rename_is_replayed() {
    let w = world();
    phone_rename(&w, "web/other", "web/renamed");
    desktop_write(&w, "web/theirs.gpg", "theirs");

    let report = integrated(&w);

    assert_eq!(report.replayed, 1);
    assert!(!phone(&w).join("web/other.gpg").exists());
    assert_eq!(read(&w, "web/renamed.gpg"), "other");
    assert_eq!(read(&w, "web/theirs.gpg"), "theirs");
    clean(&w);
}

#[test]
fn a_local_rename_of_an_entry_edited_on_the_remote_keeps_the_edit() {
    let w = world();
    phone_rename(&w, "web/other", "web/renamed");
    desktop_write(&w, "web/other.gpg", "edited on the desktop");

    let report = integrated(&w);

    assert_eq!(report.dropped_deletes, ["web/other"]);
    assert_eq!(read(&w, "web/other.gpg"), "edited on the desktop");
    assert_eq!(read(&w, "web/renamed.gpg"), "other");
    assert_eq!(report.replayed, 1);
    clean(&w);
}

#[test]
fn an_entry_edited_on_both_sides_is_kept_twice() {
    let w = world();
    let mine = phone_write(&w, "web/example.com.gpg", "v2 from the phone");
    let date = today_of(&w, &mine);
    desktop_write(&w, "web/example.com.gpg", "v2 from the desktop");

    let report = integrated(&w);

    let copy = format!("web/example.com (from this iPhone {date})");
    assert_eq!(report.kept_both, [KeptBoth { entry: "web/example.com".into(), copy: copy.clone() }]);
    assert_eq!(read(&w, "web/example.com.gpg"), "v2 from the desktop");
    assert_eq!(read(&w, &format!("{copy}.gpg")), "v2 from the phone");
    let message = git(phone(&w), &["log", "-1", "--format=%s"]);
    assert_eq!(message, format!("Keep this iPhone's version of web/example.com as {copy}."));
    clean(&w);
}

#[test]
fn keeping_both_picks_the_next_free_name() {
    let w = world();
    let mine = phone_write(&w, "web/example.com.gpg", "v2 from the phone");
    let date = today_of(&w, &mine);
    // The name is taken already, by an earlier conflict on another device.
    desktop_write(&w, &format!("web/example.com (from this iPhone {date}).gpg"), "older copy");
    desktop_write(&w, "web/example.com.gpg", "v2 from the desktop");

    let report = integrated(&w);

    let copy = format!("web/example.com (from this iPhone {date}) 2");
    assert_eq!(report.kept_both[0].copy, copy);
    assert_eq!(read(&w, &format!("{copy}.gpg")), "v2 from the phone");
    assert_eq!(read(&w, &format!("web/example.com (from this iPhone {date}).gpg")), "older copy");
    clean(&w);
}

#[test]
fn a_later_local_edit_follows_the_kept_copy() {
    let w = world();
    let mine = phone_write(&w, "web/example.com.gpg", "v2 from the phone");
    phone_write(&w, "web/example.com.gpg", "v3 from the phone");
    let date = today_of(&w, &mine);
    desktop_write(&w, "web/example.com.gpg", "v2 from the desktop");

    let report = integrated(&w);

    assert_eq!(report.kept_both.len(), 1);
    assert_eq!(report.replayed, 2);
    assert_eq!(read(&w, "web/example.com.gpg"), "v2 from the desktop");
    assert_eq!(read(&w, &format!("web/example.com (from this iPhone {date}).gpg")), "v3 from the phone");
}

#[test]
fn a_local_delete_of_an_entry_edited_on_the_remote_is_dropped() {
    let w = world();
    phone_delete(&w, "web/other");
    desktop_write(&w, "web/other.gpg", "edited on the desktop");

    let report = integrated(&w);

    assert_eq!(report.dropped_deletes, ["web/other"]);
    assert_eq!(report.replayed, 0);
    assert_eq!(read(&w, "web/other.gpg"), "edited on the desktop");
    assert_eq!(head(&w), w.server.tip());
    clean(&w);
}

#[test]
fn a_local_edit_of_an_entry_deleted_on_the_remote_is_kept() {
    let w = world();
    phone_write(&w, "web/other.gpg", "edited on the phone");
    desktop_rm(&w, "web/other.gpg");

    let report = integrated(&w);

    assert_eq!(report.kept_local, ["web/other"]);
    assert_eq!(read(&w, "web/other.gpg"), "edited on the phone");
    clean(&w);
}

#[test]
fn an_entry_deleted_on_both_sides_is_not_reported() {
    let w = world();
    phone_delete(&w, "web/other");
    let tip = desktop_rm(&w, "web/other.gpg");

    let report = integrated(&w);

    assert_eq!(report, IntegrateReport { fast_forwarded: 1, ..Default::default() });
    assert_eq!(head(&w), tip);
}

#[test]
fn a_local_commit_paw_did_not_make_is_refused_and_nothing_changes() {
    let w = world();
    std::fs::write(phone(&w).join("a.gpg"), "a").unwrap();
    std::fs::write(phone(&w).join("b.gpg"), "b").unwrap();
    phone_git(&w, &["add", "a.gpg", "b.gpg"], "Two at once, by another tool.");
    desktop_write(&w, "web/theirs.gpg", "theirs");
    let connection = Connection::open(&w.server.remote(), &signer(&w.card)).unwrap();
    steps::fetch(&connection, &w.store).unwrap();
    let before = snapshot(phone(&w));

    let error = steps::integrate(&w.store).unwrap_err();

    assert!(matches!(&error, SyncError::NotPawHistory { reason, .. } if reason.contains("more than one")), "{error:?}");
    assert_eq!(snapshot(phone(&w)), before);
}

#[test]
fn uncommitted_changes_are_refused_rather_than_overwritten() {
    let w = world();
    std::fs::write(phone(&w).join("web/example.com.gpg"), "typed in by hand").unwrap();
    desktop_write(&w, "web/example.com.gpg", "v2 from the desktop");

    let error = pull(&w).unwrap_err();

    assert!(matches!(&error, SyncError::UncommittedChanges { paths } if paths == &["web/example.com.gpg"]), "{error:?}");
    assert_eq!(read(&w, "web/example.com.gpg"), "typed in by hand");
}

#[test]
fn a_failure_in_the_middle_of_a_replay_leaves_everything_as_it_was() {
    let w = world();
    phone_write(&w, "first.gpg", "replays fine");
    phone_write(&w, "web/later.gpg", "cannot be replayed");
    // The desktop turns the folder `web` into a file, so the second local
    // commit has nowhere to go.
    desktop_git(&w, &["rm", "-r", "--quiet", "web"]);
    desktop_write(&w, "web", "a file now");
    let connection = Connection::open(&w.server.remote(), &signer(&w.card)).unwrap();
    steps::fetch(&connection, &w.store).unwrap();
    let before = snapshot(phone(&w));

    let error = steps::integrate(&w.store).unwrap_err();

    assert!(error.to_string().contains("one of its folders is a link or a file"), "{error}");
    assert_eq!(snapshot(phone(&w)), before);
}

#[test]
fn a_failure_while_updating_the_working_copy_is_rolled_back_byte_for_byte() {
    use std::os::unix::fs::PermissionsExt;
    let w = world();
    phone_write(&w, "mine.gpg", "mine");
    desktop_write(&w, "web/example.com.gpg", "v2 from the desktop");
    desktop_write(&w, "locked/new.gpg", "cannot be written");
    let connection = Connection::open(&w.server.remote(), &signer(&w.card)).unwrap();
    steps::fetch(&connection, &w.store).unwrap();
    // A folder the checkout cannot write into, as a full disk would.
    std::fs::create_dir(phone(&w).join("locked")).unwrap();
    std::fs::set_permissions(phone(&w).join("locked"), std::fs::Permissions::from_mode(0o500)).unwrap();
    let before = snapshot(phone(&w));

    let result = steps::integrate(&w.store);

    let _ = std::fs::set_permissions(phone(&w).join("locked"), std::fs::Permissions::from_mode(0o700));
    let error = result.unwrap_err();
    assert!(error.to_string().contains("ermission denied"), "{error}");
    assert_eq!(snapshot(phone(&w)), before);
    assert_eq!(read(&w, "web/example.com.gpg"), "v1 from the desktop");
}

#[test]
fn replaced_remote_history_is_not_integrated_until_the_person_decides() {
    let w = world();
    phone_write(&w, "web/mine.gpg", "mine");
    // The desktop starts over: a new history that shares nothing.
    desktop_git(&w, &["checkout", "--quiet", "--orphan", "fresh"]);
    desktop_git(&w, &["rm", "-r", "--quiet", "--cached", "."]);
    std::fs::write(w.desktop.path().join(".gpg-id"), "AAAA1111\n").unwrap();
    desktop_git(&w, &["add", ".gpg-id"]);
    desktop_git(&w, &["commit", "--quiet", "-m", "Start over."]);
    desktop_git(&w, &["push", "--quiet", "--force", "origin", "HEAD:refs/heads/main"]);
    let new_tip = w.server.tip();
    let connection = Connection::open(&w.server.remote(), &signer(&w.card)).unwrap();
    steps::fetch(&connection, &w.store).unwrap();
    let before = snapshot(phone(&w));

    let outcome = steps::integrate(&w.store).unwrap();

    match outcome {
        Integration::RemoteReplaced(replaced) => {
            assert_eq!(replaced.remote, new_tip);
            assert_eq!(replaced.unpushed, 1);
        }
        other => panic!("expected RemoteReplaced, got {other:?}"),
    }
    assert_eq!(snapshot(phone(&w)), before);

    // "Use the server's store and keep my changes."
    let report = sync::accept_replaced_remote(&w.store, true).unwrap();
    assert_eq!(report.replayed, 1);
    assert_eq!(git(phone(&w), &["rev-parse", "HEAD~1"]), new_tip);
    assert_eq!(read(&w, "web/mine.gpg"), "mine");
    assert!(!phone(&w).join("web/example.com.gpg").exists());
    clean(&w);
    // Seen now: the next pull integrates normally.
    assert!(matches!(pull(&w).unwrap(), Integration::Integrated(_)));
}

#[test]
fn replaced_remote_history_can_be_taken_without_the_local_changes() {
    let w = world();
    phone_write(&w, "web/mine.gpg", "mine");
    desktop_git(&w, &["reset", "--quiet", "--hard", "HEAD~1"]);
    desktop_git(&w, &["push", "--quiet", "--force", "origin", "HEAD:refs/heads/main"]);
    let new_tip = w.server.tip();
    assert!(matches!(pull(&w).unwrap(), Integration::RemoteReplaced(_)));

    sync::accept_replaced_remote(&w.store, false).unwrap();

    assert_eq!(head(&w), new_tip);
    assert!(!phone(&w).join("web/mine.gpg").exists());
    assert!(!phone(&w).join(".gpg-id").exists());
    clean(&w);
}

#[test]
fn a_replaced_remote_can_be_taken_over_a_commit_paw_did_not_make() {
    let w = world();
    std::fs::write(phone(&w).join("web/example.com.gpg"), "edited by hand").unwrap();
    std::fs::write(phone(&w).join("web/other.gpg"), "this one too").unwrap();
    phone_git(&w, &["add", "-A"], "Two entries at once.");
    desktop_git(&w, &["reset", "--quiet", "--hard", "HEAD~1"]);
    desktop_git(&w, &["push", "--quiet", "--force", "origin", "HEAD:refs/heads/main"]);
    let new_tip = w.server.tip();

    match pull(&w).unwrap() {
        Integration::RemoteReplaced(replaced) => assert_eq!(replaced.unpushed, 1),
        other => panic!("expected RemoteReplaced, got {other:?}"),
    }

    sync::accept_replaced_remote(&w.store, false).unwrap();
    assert_eq!(head(&w), new_tip);
    clean(&w);
}

#[test]
fn a_change_to_a_gpg_id_is_reported_with_both_versions() {
    let w = world();
    phone_write(&w, "web/mine.gpg", "mine");
    desktop_write(&w, ".gpg-id", "AAAA1111\nBBBB2222 # the backup key\n");
    desktop_write(&w, "work/.gpg-id", "CCCC3333\n");

    let report = integrated(&w);

    assert_eq!(
        report.recipients_changed,
        [
            RecipientsChange { path: ".gpg-id".into(), before: vec!["AAAA1111".into()], after: vec!["AAAA1111".into(), "BBBB2222".into()] },
            RecipientsChange { path: "work/.gpg-id".into(), before: vec![], after: vec!["CCCC3333".into()] },
        ]
    );
    // Reported, and integrated all the same.
    assert_eq!(report.replayed, 1);
}

#[test]
fn a_gpg_id_that_is_not_utf8_is_reported_as_far_as_it_reads() {
    let w = world();
    std::fs::write(w.desktop.path().join(".gpg-id"), b"AAAA1111\nBBBB\xff2222\n").unwrap();
    desktop_push(&w, "Edit .gpg-id on the desktop.");

    let report = integrated(&w);

    assert_eq!(
        report.recipients_changed,
        [RecipientsChange { path: ".gpg-id".into(), before: vec!["AAAA1111".into()], after: vec!["AAAA1111".into(), "BBBB\u{FFFD}2222".into()] }]
    );
}

// --- one sync, one login ---------------------------------------------------

#[test]
fn a_sync_fetches_replays_and_pushes_with_one_signature() {
    let w = world();
    phone_write(&w, "web/mine.gpg", "mine");
    phone_write(&w, "web/example.com.gpg", "v2 from the phone");
    desktop_write(&w, "web/theirs.gpg", "theirs");
    let signatures = w.card.authentication_count();

    let report = steps::sync(&w.server.remote(), &signer(&w.card), &w.store, true).unwrap();

    assert_eq!(report.state, SyncState::UpToDate);
    assert_eq!((report.integration.replayed, report.pushed, report.retried), (2, 2, false));
    assert_eq!(w.card.authentication_count() - signatures, 1);
    assert_eq!(w.server.tip(), head(&w));
    assert_eq!(git(&w.server.bare(), &["show", "main:web/theirs.gpg"]), "theirs");
    assert_eq!(git(&w.server.bare(), &["show", "main:web/mine.gpg"]), "mine");
    assert_eq!(w.server.services().last().unwrap(), &["git-upload-pack", "git-receive-pack"]);
}

#[test]
fn a_sync_whose_push_meets_a_moved_remote_retries_on_the_same_login() {
    let w = world();
    phone_write(&w, "web/mine.gpg", "mine");
    w.server.move_before_next_push("web/sneaky.gpg", "pushed in between");
    let logins = w.server.logins();

    let report = steps::sync(&w.server.remote(), &signer(&w.card), &w.store, true).unwrap();

    assert!(report.retried);
    assert_eq!(w.server.logins() - logins, 1);
    assert_eq!(
        w.server.services().last().unwrap(),
        &["git-upload-pack", "git-receive-pack", "git-upload-pack", "git-receive-pack"]
    );
    assert_eq!(w.server.tip(), head(&w));
    assert_eq!(git(&w.server.bare(), &["show", "main:web/sneaky.gpg"]), "pushed in between");
    assert_eq!(git(&w.server.bare(), &["show", "main:web/mine.gpg"]), "mine");
    assert_eq!(read(&w, "web/sneaky.gpg"), "pushed in between");
}

#[test]
fn a_refresh_integrates_without_pushing() {
    let w = world();
    phone_write(&w, "web/mine.gpg", "mine");
    let remote = desktop_write(&w, "web/theirs.gpg", "theirs");
    let signatures = w.card.authentication_count();

    let report = steps::sync(&w.server.remote(), &signer(&w.card), &w.store, false).unwrap();

    assert_eq!(report.state, SyncState::Waiting { unpushed: 1 });
    assert_eq!((report.pushed, w.card.authentication_count() - signatures), (0, 1));
    assert_eq!(w.server.tip(), remote);
    assert_eq!(read(&w, "web/theirs.gpg"), "theirs");
    assert_eq!(w.server.services().last().unwrap(), &["git-upload-pack"]);
}

#[test]
fn a_sync_with_nothing_to_send_pushes_nothing() {
    let w = world();
    desktop_write(&w, "web/theirs.gpg", "theirs");
    let signatures = w.card.authentication_count();

    let report = steps::sync(&w.server.remote(), &signer(&w.card), &w.store, true).unwrap();

    assert_eq!((report.state, report.pushed, w.card.authentication_count() - signatures), (SyncState::UpToDate, 0, 1));
    assert_eq!(report.integration.fast_forwarded, 1);
    assert_eq!(w.server.services().last().unwrap(), &["git-upload-pack"]);
}

#[test]
fn a_sync_onto_replaced_history_stops_before_pushing() {
    let w = world();
    phone_write(&w, "web/mine.gpg", "mine");
    desktop_git(&w, &["reset", "--quiet", "--hard", "HEAD~1"]);
    desktop_git(&w, &["push", "--quiet", "--force", "origin", "HEAD:refs/heads/main"]);
    let remote = w.server.tip();

    let report = steps::sync(&w.server.remote(), &signer(&w.card), &w.store, true).unwrap();

    assert!(matches!(report.state, SyncState::RemoteReplaced { .. }), "{report:?}");
    assert_eq!(w.server.tip(), remote);
}

#[test]
fn a_sync_with_a_key_the_host_does_not_know_stops_before_the_pin() {
    let w = world();
    let signatures = w.card.authentication_count();
    let stranger = paw_core::ssh::SshPublicKey::ed25519(&[3u8; 32]).authorized_key();
    let server = GitServer::start(&[stranger]);

    let error = sync_failure(steps::sync(&server.remote(), &signer(&w.card), &w.store, true).unwrap_err());

    assert!(matches!(error, SyncError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::KeyNotAccepted { .. } } }), "{error:?}");
    assert_eq!(w.card.authentication_count(), signatures);
    assert_eq!(w.card.pin_tries_left(), 3);
}

#[test]
fn a_sync_to_a_host_with_another_key_is_refused_with_both_fingerprints() {
    let w = world();
    let mut remote = w.server.remote();
    let real = remote.host_key.fingerprints().remove(0);
    remote.host_key = HostKeyPin::parse(&support::some_other_host_key()).unwrap();
    let pinned = remote.host_key.fingerprints().remove(0);

    let error = sync_failure(steps::sync(&remote, &signer(&w.card), &w.store, true).unwrap_err());

    match error {
        SyncError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::HostKeyChanged { expected, presented, .. } } } => {
            assert_eq!(expected, [pinned]);
            assert_eq!(presented, real);
        }
        other => panic!("expected HostKeyChanged, got {other:?}"),
    }
    assert_eq!(w.card.authentication_count(), 1);
}

// --- the unpushed queue ----------------------------------------------------

fn queued(w: &World) -> Vec<(ChangeKind, String, Option<String>)> {
    queue::unpushed(&w.store)
        .unwrap()
        .into_iter()
        .map(|item| (item.kind, item.entry, item.previous))
        .collect()
}

fn four_changes(w: &World) {
    phone_write(w, "web/new.gpg", "new");
    phone_write(w, "web/example.com.gpg", "v2 from the phone");
    phone_delete(w, "web/other");
    phone_rename(w, "web/new", "web/newer");
}

#[test]
fn a_pushed_change_leaves_the_queue() {
    let w = world();
    four_changes(&w);

    steps::sync(&w.server.remote(), &signer(&w.card), &w.store, true).unwrap();

    assert!(queued(&w).is_empty());
}

#[test]
fn the_queue_never_touches_pushed_history() {
    let w = world();
    let pushed = head(&w);
    phone_write(&w, "web/mine.gpg", "mine");
    let before = snapshot(phone(&w));

    let error = queue::discard(&w.store, &pushed).unwrap_err();

    assert!(error.to_string().contains("not an unpushed change"), "{error}");
    assert_eq!(snapshot(phone(&w)), before);
}

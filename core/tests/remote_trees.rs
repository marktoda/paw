//! A hostile remote's trees: links, submodules, unsafe names and names that
//! are one name on an iPhone are refused before anything is checked out,
//! whether they arrive by clone, by fetch and fast-forward, or under
//! changes waiting to be pushed. The trees are written straight into the
//! host's object database, as a crafted push would leave them.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use git2::{Oid, Repository, Signature};
use paw_core::sim::remote::steps;
use paw_core::sim::SimulatedKey;
use paw_core::fault::Fault;
use paw_core::remote::{RefusedPath, RefusedReason, RemoteFailure};
use paw_core::sync::{self, Author, Cloned, Connection, SyncError, queue};
use key_common::{PIN, signer};
use paw_core::files::layout::StoreDirs;
use support::{GitServer, Phone, git};


/// One entry of a crafted tree.
enum Node {
    File(&'static [u8]),
    Executable(&'static [u8]),
    Link(&'static str),
    Submodule,
    Folder(Vec<(&'static str, Node)>),
}
use Node::{Executable, File, Folder, Link, Submodule};

/// Writes `entries` as a raw tree object: no name or mode is checked, as
/// a hand-made push would not be.
fn write_tree(repo: &Repository, entries: &[(&str, Node)]) -> Oid {
    let mut written: Vec<(Vec<u8>, &str, Oid)> = entries
        .iter()
        .map(|(name, node)| {
            let (mode, id) = match node {
                File(bytes) => ("100644", repo.blob(bytes).unwrap()),
                Executable(bytes) => ("100755", repo.blob(bytes).unwrap()),
                Link(target) => ("120000", repo.blob(target.as_bytes()).unwrap()),
                // Any commit id will do: it is never fetched.
                Submodule => ("160000", Oid::from_str("0123456789abcdef0123456789abcdef01234567").unwrap()),
                Folder(children) => ("40000", write_tree(repo, children)),
            };
            // git sorts a folder as if its name ended in a slash.
            let mut key = name.as_bytes().to_vec();
            if matches!(node, Folder(_)) {
                key.push(b'/');
            }
            (key, mode, id)
        })
        .collect();
    written.sort_by(|a, b| a.0.cmp(&b.0));
    let mut bytes = Vec::new();
    for (key, mode, id) in written {
        let name = key.strip_suffix(b"/").unwrap_or(&key);
        bytes.extend_from_slice(mode.as_bytes());
        bytes.push(b' ');
        bytes.extend_from_slice(name);
        bytes.push(0);
        bytes.extend_from_slice(id.as_bytes());
    }
    repo.odb().unwrap().write(git2::ObjectType::Tree, &bytes).unwrap()
}

/// What every crafted tree holds besides the bad part: a small store.
fn store(extra: Vec<(&'static str, Node)>) -> Vec<(&'static str, Node)> {
    let mut entries = vec![
        (".gpg-id", File(b"0123456789ABCDEF0123456789ABCDEF01234567\n")),
        ("README", File(b"a password store\n")),
    ];
    let mut web = vec![("example.com.gpg", File(b"ciphertext"))];
    let mut rest = Vec::new();
    for (name, node) in extra {
        match (name, node) {
            ("web", Folder(children)) => web.extend(children),
            other => rest.push(other),
        }
    }
    entries.push(("web", Folder(web)));
    entries.extend(rest);
    entries
}

/// Commits `entries` on the host's `main`, on top of what is there.
fn host_commit(server: &GitServer, entries: &[(&str, Node)]) -> String {
    let repo = Repository::open_bare(server.bare()).unwrap();
    let tree = repo.find_tree(write_tree(&repo, entries)).unwrap();
    let parent = repo.refname_to_id("refs/heads/main").ok().map(|id| repo.find_commit(id).unwrap());
    let signature = Signature::now("Desktop", "desktop@example.com").unwrap();
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let id = repo.commit(Some("refs/heads/main"), &signature, &signature, "Crafted.", &tree, &parents).unwrap();
    id.to_string()
}

/// What a case is, the entries it adds to the store, and a path the
/// refusal must name.
type Hostile = (&'static str, Vec<(&'static str, Node)>, &'static str);

/// The bad parts.
fn hostile_trees() -> Vec<Hostile> {
    vec![
        ("a link", vec![("up", Link(".."))], "up"),
        ("a link in a folder", vec![("web", Folder(vec![("shared", Link("/etc"))]))], "web/shared"),
        ("a submodule", vec![("vendored", Submodule)], "vendored"),
        (
            "an NFC and an NFD name",
            vec![("caf\u{e9}.gpg", File(b"one")), ("cafe\u{301}.gpg", File(b"two"))],
            "caf",
        ),
        ("two letter cases", vec![("web", Folder(vec![("GitHub.gpg", File(b"one")), ("github.gpg", File(b"two"))]))], "web/"),
        ("a file and a folder of one name", vec![("Web", File(b"x"))], "Web"),
        ("an NFC link and an NFD folder", vec![("caf\u{e9}", Link("..")), ("cafe\u{301}", Folder(vec![("recipients", File(b"planted"))]))], "caf"),
        (".GIT", vec![(".GIT", Folder(vec![("config", File(b"[core]\n"))]))], ".GIT"),
        (
            "a .git with a code point HFS+ ignores",
            vec![(".g\u{200c}it", Folder(vec![("hooks", Folder(vec![("post-checkout", Executable(b"#!/bin/sh\n"))]))]))],
            ".g\u{200c}it",
        ),
        ("the NTFS short name of .git", vec![("GIT~1", Folder(vec![("config", File(b"[core]\n"))]))], "GIT~1"),
        ("a .gpg-id in another spelling", vec![("web", Folder(vec![(".GPG-ID", File(b"FFFF\n"))]))], "web/.GPG-ID"),
        ("a .gpg-id with an ignorable code point", vec![("web", Folder(vec![(".gpg-id\u{200d}", File(b"FFFF\n"))]))], "web/.gpg-id"),
        ("a .. name", vec![("..", Folder(vec![("recipients", File(b"planted"))]))], ".."),
    ]
}

fn card() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), false, 50)
}

fn server(card: &Arc<SimulatedKey>) -> GitServer {
    GitServer::start(&[card.ssh_public_key()])
}

fn connect(server: &GitServer, card: &Arc<SimulatedKey>) -> Connection {
    Connection::open(&server.remote(), &signer(card)).unwrap()
}

/// Every path under `dir` but `.git`, as a set, and the paths a link
/// out of the store could have reached.
fn files(dir: &Path) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                folders.push(path.clone());
            }
            found.insert(path.strip_prefix(dir).unwrap().to_path_buf());
        }
    }
    found
}

fn author() -> Author {
    Author { name: "iPhone".into(), email: "iphone@example.com".into() }
}

fn assert_refusal_names(paths: &[RefusedPath], path: &str, what: &str) {
    let names = |refused: &RefusedPath| {
        refused.path.starts_with(path)
            || refused.reasons.iter().any(|reason| matches!(reason, RefusedReason::SameNameAs { first } if first.starts_with(path)))
    };
    assert!(paths.iter().any(names), "{what}: {paths:?} does not name {path}");
}

/// What a sync refused because of the remote's content.
fn refused_content<T: std::fmt::Debug>(result: Result<T, SyncError>) -> Vec<RefusedPath> {
    match result {
        Err(SyncError::Fault { fault: Fault::Remote { failure: RemoteFailure::RefusedContent { paths } } }) => paths,
        other => panic!("not refused for the remote's content: {other:?}"),
    }
}

/// What is in the state directory, apart from paw's lock files.
fn beside(state: &Path) -> Vec<String> {
    let locks = [paw_core::files::layout::STORE_LOCK];
    let mut names: Vec<String> = std::fs::read_dir(state)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !locks.contains(&name.as_str()))
        .collect();
    names.sort();
    names
}

#[test]
fn a_clone_of_a_hostile_tree_checks_out_nothing() {
    let card = card();
    for (what, extra, path) in hostile_trees() {
        let server = server(&card);
        host_commit(&server, &store(extra));
        let state = tempfile::tempdir().unwrap();
        let dir = state.path().join(paw_core::files::layout::REPO);
        let dirs = StoreDirs::new(state.path());

        let result = steps::clone(&connect(&server, &card), &server.remote(), &dirs);

        let Ok(Cloned::Refused(paths)) = result else {
            panic!("{what}: the clone was not refused: {result:?}");
        };
        assert_refusal_names(&paths, path, what);
        assert_eq!(files(&dir), BTreeSet::new(), "{what}: something was checked out");
        assert_eq!(beside(state.path()), [paw_core::files::layout::REPO], "{what}: wrote beside the store");
    }
}

/// A clone of a good store, then `extra` pushed by the desktop, then a
/// fetch; with a change waiting to be pushed if `queued`. Returns what
/// integrate said and checks that nothing moved.
fn integrate_after(
    card: &Arc<SimulatedKey>,
    what: &str,
    extra: Vec<(&'static str, Node)>,
    queued: bool,
) -> Vec<RefusedPath> {
    let server = server(card);
    host_commit(&server, &store(Vec::new()));
    let state = tempfile::tempdir().unwrap();
    let dir = state.path().join(paw_core::files::layout::REPO);
    let dirs = StoreDirs::new(state.path());
    let connection = connect(&server, card);
    assert_eq!(steps::clone(&connection, &server.remote(), &dirs).unwrap(), Cloned::Store);
    if queued {
        sync::commit(&dirs, "web/new.gpg", b"local ciphertext", "Add given password for web/new to store.", &author())
            .unwrap();
    }
    let head = git(&dir, &["rev-parse", "HEAD"]);
    let before = files(&dir);
    let queue = queue::unpushed(&dirs).unwrap();

    host_commit(&server, &store(extra));
    steps::fetch(&connection, &dirs).unwrap();
    let result = steps::integrate(&dirs);

    let paths = refused_content(result);
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head, "{what}");
    assert_eq!(files(&dir), before, "{what}: the working copy changed");
    assert_eq!(git(&dir, &["status", "--porcelain"]), "", "{what}");
    assert_eq!(queue::unpushed(&dirs).unwrap(), queue, "{what}: the queue changed");
    assert_eq!(beside(state.path()), [paw_core::files::layout::REPO], "{what}: wrote beside the store");
    // The app can say what was refused, from what was fetched.
    let refused = sync::refused_remote(&dirs).unwrap();
    assert!(!refused.is_empty(), "{what}");
    paths
}

#[test]
fn a_fast_forward_to_a_hostile_tree_is_refused() {
    let card = card();
    for (what, extra, path) in hostile_trees() {
        let paths = integrate_after(&card, what, extra, false);
        assert_refusal_names(&paths, path, what);
    }
}

#[test]
fn a_hostile_tree_under_waiting_changes_is_refused_and_the_queue_kept() {
    let card = card();
    for (what, extra, path) in hostile_trees() {
        let paths = integrate_after(&card, what, extra, true);
        assert_refusal_names(&paths, path, what);
    }
}

#[test]
fn a_good_tree_passes_and_names_nothing() {
    let card = card();
    let server = server(&card);
    // Lookalikes that are fine: different names on every file system.
    host_commit(
        &server,
        &store(vec![
            ("web", Folder(vec![(".github.gpg", File(b"x")), ("caf\u{e9}.gpg", File(b"x")), ("a.", File(b"x"))])),
            (".extensions", Folder(vec![("otp.bash", Executable(b"#!/bin/sh\n"))])),
            (".gitattributes", File(b"*.gpg diff=gpg\n")),
        ]),
    );
    let dir = Phone::new();
    assert_eq!(steps::clone(&connect(&server, &card), &server.remote(), &dir.store).unwrap(), Cloned::Store);
    assert!(dir.path().join("web/caf\u{e9}.gpg").is_file());
    assert!(sync::refused_remote(&dir.store).unwrap().is_empty());
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
}

#[test]
fn a_local_name_that_collides_with_a_remote_one_is_not_replayed() {
    let card = card();
    let server = server(&card);
    host_commit(&server, &store(Vec::new()));
    let dir = Phone::new();
    let connection = connect(&server, &card);
    assert_eq!(steps::clone(&connection, &server.remote(), &dir.store).unwrap(), Cloned::Store);
    // This device saves `Bank.gpg`; the desktop, meanwhile, `bank.gpg`.
    sync::commit(&dir.store, "web/Bank.gpg", b"mine", "Add given password for web/Bank to store.", &author()).unwrap();
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    host_commit(&server, &store(vec![("web", Folder(vec![("bank.gpg", File(b"theirs"))]))]));
    steps::fetch(&connection, &dir.store).unwrap();

    let result = steps::integrate(&dir.store);

    assert_refusal_names(&refused_content(result), "web/bank.gpg", "replay");
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(queue::unpushed(&dir.store).unwrap().len(), 1);
    // The remote alone is fine; the collision is with this device's change.
    assert!(sync::refused_remote(&dir.store).unwrap().is_empty());
}

#[test]
fn saving_a_name_that_collides_with_one_in_the_store_is_refused() {
    let card = card();
    let server = server(&card);
    host_commit(&server, &store(Vec::new()));
    let dir = Phone::new();
    assert_eq!(steps::clone(&connect(&server, &card), &server.remote(), &dir.store).unwrap(), Cloned::Store);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);

    for path in ["Web/x.gpg", "web/Example.com.gpg", "WEB"] {
        let result = sync::commit(&dir.store, path, b"x", "Add.", &author());
        assert!(matches!(&result, Err(Fault::Refused { .. })), "{path}: {result:?}");
    }
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
}

/// Finding 13: the remote turns a folder that holds a waiting change into a
/// link. The person sees the refusal, the change stays queued, and nothing
/// is written through the link.
#[test]
fn a_folder_turned_into_a_link_under_a_waiting_change_is_refused() {
    let card = card();
    let server = server(&card);
    host_commit(&server, &store(Vec::new()));
    let state = tempfile::tempdir().unwrap();
    let dir = state.path().join(paw_core::files::layout::REPO);
    let dirs = StoreDirs::new(state.path());
    let connection = connect(&server, &card);
    assert_eq!(steps::clone(&connection, &server.remote(), &dirs).unwrap(), Cloned::Store);
    sync::commit(&dirs, "web/new.gpg", b"mine", "Add given password for web/new to store.", &author()).unwrap();

    let repo = Repository::open_bare(server.bare()).unwrap();
    let tree = write_tree(
        &repo,
        &[(".gpg-id", File(b"0123456789ABCDEF0123456789ABCDEF01234567\n")), ("web", Link(".."))],
    );
    let parent = repo.find_commit(repo.refname_to_id("refs/heads/main").unwrap()).unwrap();
    let signature = Signature::now("Desktop", "desktop@example.com").unwrap();
    repo.commit(Some("refs/heads/main"), &signature, &signature, "Link.", &repo.find_tree(tree).unwrap(), &[&parent])
        .unwrap();
    steps::fetch(&connection, &dirs).unwrap();

    let result = steps::integrate(&dirs);

    let link = vec![RefusedPath { path: "web".into(), reasons: vec![RefusedReason::SymbolicLink] }];
    assert_eq!(refused_content(result), link);
    assert_eq!(sync::refused_remote(&dirs).unwrap(), link);
    assert_eq!(queue::unpushed(&dirs).unwrap().len(), 1);
    assert!(dir.join("web").is_dir() && !dir.join("web").is_symlink());
    assert_eq!(std::fs::read(dir.join("web/new.gpg")).unwrap(), b"mine");
    assert!(!state.path().join("new.gpg").exists());
}

// The reviewer's scenario on a real case- and normalisation-insensitive file
// system: a Linux tmpfs mounted with `casefold`, in a user namespace so no
// privilege is needed. The test runs itself again inside the namespace.

const CASEFOLD_DIR: &str = "PAW_TEST_CASEFOLD_DIR";

/// Runs `test` (this binary's test of that name) inside a user and mount
/// namespace with a casefold tmpfs, or says why it cannot.
fn in_casefold_namespace(test: &str) -> bool {
    let mount = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let m = mount.path().display();
    // 77: the namespace or the file system is not available here.
    let script = format!(
        "mount -t tmpfs -o casefold tmpfs '{m}' || exit 77; mkdir '{m}/cf' && chattr +F '{m}/cf' || exit 77; \
         {CASEFOLD_DIR}='{m}/cf' exec '{}' --exact {test} --nocapture --test-threads=1",
        exe.display()
    );
    // Some systems (GitHub's runners among them) forbid user namespaces.
    let allowed = std::process::Command::new("unshare")
        .args(["-rm", "true"])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !allowed {
        eprintln!("skipped: user namespaces are not allowed here");
        return false;
    }
    match std::process::Command::new("unshare").args(["-rm", "sh", "-c", &script]).status() {
        Ok(status) if status.code() == Some(77) => {
            eprintln!("skipped: no casefold tmpfs in a user namespace here");
            false
        }
        Ok(status) => {
            assert!(status.success(), "the test failed on the casefold file system");
            true
        }
        Err(e) => {
            eprintln!("skipped: unshare is not available: {e}");
            false
        }
    }
}

fn commit_tree(repo: &Repository, parent: Option<Oid>, entries: &[(&str, Node)]) -> Oid {
    let tree = repo.find_tree(write_tree(repo, entries)).unwrap();
    let signature = Signature::now("Desktop", "desktop@example.com").unwrap();
    let parents: Vec<git2::Commit> = parent.iter().map(|p| repo.find_commit(*p).unwrap()).collect();
    let parents: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(None, &signature, &signature, "Remote.", &tree, &parents).unwrap()
}

#[test]
fn a_pull_cannot_write_beside_the_store_on_a_casefold_file_system() {
    let Ok(base) = std::env::var(CASEFOLD_DIR) else {
        in_casefold_namespace("a_pull_cannot_write_beside_the_store_on_a_casefold_file_system");
        return;
    };
    // Inside the namespace, on the casefold file system.
    let base = Path::new(&base);
    let probe = base.join("Caf\u{e9}");
    std::fs::write(&probe, "x").unwrap();
    assert!(base.join("CAFE\u{301}").exists(), "the file system is not normalisation-insensitive");
    std::fs::remove_file(probe).unwrap();

    let state = base.join("state");
    let dir = state.join(paw_core::files::layout::REPO);
    std::fs::create_dir_all(&dir).unwrap();
    let dirs = StoreDirs::new(&state);
    // A copy made before paw refused links: the link is on disk already.
    let repo = Repository::init(&dir).unwrap();
    repo.config().unwrap().set_bool("core.ignorecase", false).unwrap();
    let first = commit_tree(
        &repo,
        None,
        &[(".gpg-id", File(b"0123456789ABCDEF0123456789ABCDEF01234567\n")), ("caf\u{e9}", Link(".."))],
    );
    for name in ["refs/heads/main", "refs/remotes/origin/main", "refs/paw/last-seen/main"] {
        repo.reference(name, first, true, "old clone").unwrap();
    }
    repo.set_head("refs/heads/main").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    assert!(dir.join("caf\u{e9}").is_symlink());
    // The remote then adds `café/recipients`, the folder's name in NFD.
    let second = commit_tree(
        &repo,
        Some(first),
        &[
            (".gpg-id", File(b"0123456789ABCDEF0123456789ABCDEF01234567\n")),
            ("caf\u{e9}", Link("..")),
            ("cafe\u{301}", Folder(vec![("recipients", File(b"# planted by the remote\n"))])),
        ],
    );
    repo.reference("refs/remotes/origin/main", second, true, "fetch").unwrap();

    let result = steps::integrate(&dirs);

    assert!(!state.join(paw_core::files::layout::RECIPIENTS).exists(), "a pull wrote outside the store: {result:?}");
    // The NFC link and the NFD folder are one name here.
    let named = RefusedPath {
        path: "caf\u{e9}".into(),
        reasons: vec![RefusedReason::SymbolicLink, RefusedReason::SameNameAs { first: "cafe\u{301}".into() }],
    };
    let paths = refused_content(result);
    assert!(paths.contains(&named), "{paths:?}");

    // Without the link, the two spellings of one name are refused too: on
    // this file system the second would overwrite the first.
    let third = commit_tree(
        &repo,
        Some(first),
        &[
            (".gpg-id", File(b"0123456789ABCDEF0123456789ABCDEF01234567\n")),
            ("caf\u{e9}.gpg", File(b"one")),
            ("cafe\u{301}.gpg", File(b"two")),
        ],
    );
    repo.reference("refs/remotes/origin/main", third, true, "fetch").unwrap();
    let result = steps::integrate(&dirs);
    let paths = refused_content(result);
    let same_name = |reason: &RefusedReason| matches!(reason, RefusedReason::SameNameAs { .. });
    assert!(paths.iter().any(|refused| refused.reasons.iter().any(same_name)), "{paths:?}");
    assert!(!state.join(paw_core::files::layout::RECIPIENTS).exists());
}

#[test]
fn a_clone_on_a_casefold_file_system_writes_no_link() {
    let Ok(base) = std::env::var(CASEFOLD_DIR) else {
        in_casefold_namespace("a_clone_on_a_casefold_file_system_writes_no_link");
        return;
    };
    let card = card();
    let server = server(&card);
    host_commit(&server, &store(vec![("caf\u{e9}", Link(".."))]));
    let state = Path::new(&base).join("state");
    let dir = state.join(paw_core::files::layout::REPO);
    let dirs = StoreDirs::new(&state);

    let result = steps::clone(&connect(&server, &card), &server.remote(), &dirs);

    let Ok(Cloned::Refused(paths)) = result else { panic!("{result:?}") };
    assert!(!paths.is_empty());
    assert!(!dir.join("caf\u{e9}").exists());
    assert_eq!(files(&dir), BTreeSet::new());
}

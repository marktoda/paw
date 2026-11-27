//! The spike's exit test: clone, commit and push a password store over SSH,
//! with every login signed by a simulated OpenPGP card. The git host is an
//! in-process SSH server in front of the real git programs (see `support`).

#![cfg(feature = "sim")]

mod key_common;
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use key_common::{Driver, KeySigner, PIN, login, port_with};
use paw_core::sim::remote::steps;
use paw_core::key::KeyError;
use paw_core::task::LoginPurpose;
use paw_core::sim::SimulatedKey;
use paw_core::key::{SignFailure, SshSigner};
use paw_core::ssh::SshPublicKey;
use paw_core::fault::Fault;
use paw_core::remote::RemoteFailure;
use paw_core::sync::{self, Author, Cloned, Connection, HostKeyPin, SyncError};
use support::{GitServer, Phone, git};

const ENTRY: &str = "web/example.com.gpg";
const MESSAGE: &str = "Add given password for web/example.com to store.";

/// A card whose authentication slot needs a touch for every signature.
fn card() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), true, 50)
}

fn authorised_line(card: &Arc<SimulatedKey>) -> String {
    card.ssh_public_key()
}

/// A signer that touches the card whenever it asks.
fn signer(card: &Arc<SimulatedKey>) -> KeySigner {
    key_common::signer(card)
}

fn connect(server: &GitServer, card: &Arc<SimulatedKey>) -> Connection {
    Connection::open(&server.remote(), &signer(card)).unwrap()
}

fn author() -> Author {
    Author { name: "iPhone".into(), email: "iphone@example.com".into() }
}

fn commit_entry(dir: &Phone) -> String {
    sync::commit(&dir.store, ENTRY, b"ciphertext", MESSAGE, &author()).unwrap()
}

/// `Connection` has no `Debug`, so `unwrap_err` is not available.
fn refusal(result: Result<Connection, sync::ConnectError>) -> RemoteFailure {
    match result.err().expect("the connection should have been refused") {
        sync::ConnectError::Fault(Fault::Remote { failure }) => failure,
        sync::ConnectError::Fault(error) => panic!("expected the host's refusal, got {error:?}"),
        sync::ConnectError::Key(error) => panic!("expected the host's refusal, got the key's: {error:?}"),
        sync::ConnectError::Cancelled => panic!("expected the host's refusal, got a cancel"),
    }
}

fn key_refusal(result: Result<Connection, sync::ConnectError>) -> KeyError {
    match result.err().expect("the connection should have been refused") {
        sync::ConnectError::Key(error) => error,
        sync::ConnectError::Fault(error) => panic!("expected the key's refusal, got {error:?}"),
        sync::ConnectError::Cancelled => panic!("expected the key's refusal, got a cancel"),
    }
}

#[test]
fn clones_commits_and_pushes_with_the_card() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = Phone::new();

    let connection = connect(&server, &card);
    assert_eq!(steps::clone(&connection, &server.remote(), &dir.store).unwrap(), Cloned::Store);
    drop(connection);

    assert_eq!(std::fs::read_to_string(dir.path().join("README")).unwrap(), "a password store\n");
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), server.tip());
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
    // A clone is one login, so one touch.
    assert_eq!(card.authentication_count(), 1);

    let commit = commit_entry(&dir);
    assert_eq!(std::fs::read(dir.path().join(ENTRY)).unwrap(), b"ciphertext");
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
    assert_eq!(git(dir.path(), &["log", "-1", "--format=%s|%an|%ae"]), format!("{MESSAGE}|iPhone|iphone@example.com"));
    assert_eq!(git(dir.path(), &["show", "--format=", "--name-only", "HEAD"]), ENTRY);
    assert_ne!(server.tip(), commit);

    let connection = connect(&server, &card);
    steps::push(&connection, &dir.store).unwrap();

    assert_eq!(server.tip(), commit);
    assert_eq!(git(&server.bare(), &["show", &format!("main:{ENTRY}")]), "ciphertext");
    assert_eq!(git(dir.path(), &["rev-parse", "origin/main"]), commit);
    assert_eq!(card.authentication_count(), 2);
}

#[test]
fn fetch_and_push_share_one_login() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = Phone::new();
    assert_eq!(steps::clone(&connect(&server, &card), &server.remote(), &dir.store).unwrap(), Cloned::Store);
    let commit = commit_entry(&dir);
    let touches_before = card.authentication_count();

    let connection = connect(&server, &card);
    let remote_tip = steps::fetch(&connection, &dir.store).unwrap();
    steps::push(&connection, &dir.store).unwrap();

    assert_ne!(remote_tip, commit);
    assert_eq!(server.tip(), commit);
    assert_eq!(card.authentication_count() - touches_before, 1);
    // The second login ran both services, each in its own channel.
    assert_eq!(server.logins(), 2);
    assert_eq!(server.services()[1], ["git-upload-pack", "git-receive-pack"]);
}

#[test]
fn fetch_brings_in_what_the_desktop_pushed() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = Phone::new();
    let connection = connect(&server, &card);
    assert_eq!(steps::clone(&connection, &server.remote(), &dir.store).unwrap(), Cloned::Store);

    let desktop = server.desktop_commit("desktop.gpg", "from the desktop");
    let remote_tip = steps::fetch(&connection, &dir.store).unwrap();

    assert_eq!(remote_tip, desktop);
    assert_eq!(git(dir.path(), &["rev-parse", "origin/main"]), desktop);
    // Fetching does not move the local branch or the working copy.
    assert_ne!(git(dir.path(), &["rev-parse", "HEAD"]), desktop);
    // Clone and fetch on one connection: still one touch.
    assert_eq!(card.authentication_count(), 1);
}

#[test]
fn a_push_onto_a_remote_that_moved_is_refused_and_changes_nothing() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = Phone::new();
    let connection = connect(&server, &card);
    assert_eq!(steps::clone(&connection, &server.remote(), &dir.store).unwrap(), Cloned::Store);

    let desktop = server.desktop_commit("desktop.gpg", "from the desktop");
    commit_entry(&dir);

    // Before fetching, the remote's new tip is unknown here.
    assert!(matches!(steps::push(&connection, &dir.store), Err(sync::PushError::NotFastForward)));
    // After fetching it is known, and still not an ancestor of ours.
    steps::fetch(&connection, &dir.store).unwrap();
    assert!(matches!(steps::push(&connection, &dir.store), Err(sync::PushError::NotFastForward)));

    assert_eq!(server.tip(), desktop);
}

#[test]
fn a_push_the_host_refuses_is_reported() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = Phone::new();
    let connection = connect(&server, &card);
    assert_eq!(steps::clone(&connection, &server.remote(), &dir.store).unwrap(), Cloned::Store);
    let before = server.tip();

    // As a pre-receive hook that exits with an error would.
    server.sim.set_refuse_pushes(true);
    commit_entry(&dir);

    match steps::push(&connection, &dir.store) {
        Err(sync::PushError::Failed(Fault::Remote { failure: RemoteFailure::PushRefused { detail: message } })) => {
            assert!(message.contains("refused the push"), "{message}")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(server.tip(), before);
    assert_eq!(git(dir.path(), &["rev-parse", "origin/main"]), before);
}

#[test]
fn a_key_the_host_does_not_know_is_rejected_before_the_card_is_used() {
    let card = card();
    let someone_else = SshPublicKey::ed25519(&[3u8; 32]).authorized_key();
    let server = GitServer::start(&[someone_else]);

    let error = refusal(Connection::open(&server.remote(), &signer(&card)));

    let ours = authorised_line(&card);
    assert_eq!(error, RemoteFailure::KeyNotAccepted { host: "127.0.0.1".into(), public_key: ours });
    // No signature, so no touch.
    assert_eq!(card.authentication_count(), 0);
    assert_eq!(server.logins(), 0);
}

#[test]
fn a_host_with_the_wrong_key_is_refused_before_login() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let mut remote = server.remote();
    let real = remote.host_key.fingerprint();
    remote.host_key = HostKeyPin::parse(&support::some_other_host_key()).unwrap();

    let error = refusal(Connection::open(&remote, &signer(&card)));

    match error {
        RemoteFailure::HostKeyChanged { presented, bundled, .. } => assert_eq!((presented, bundled), (real, false)),
        other => panic!("expected HostKeyChanged, got {other:?}"),
    }
    assert_eq!(card.authentication_count(), 0);
    assert_eq!(server.logins(), 0);
}

#[test]
fn an_untouched_key_fails_the_login() {
    // Longer than a quick refusal: the key really waited for a touch.
    let card = SimulatedKey::new(PIN.into(), true, 1_200);
    let server = GitServer::start(&[authorised_line(&card)]);
    // The PIN is typed, the touch never comes.
    let port = port_with(&card);
    let login = login(&port, LoginPurpose::Fetch, &Driver::typist(PIN));
    let signer = login.signer().unwrap();

    let error = key_refusal(Connection::open(&server.remote(), &signer));

    assert_eq!(error, KeyError::TouchTimeout);
    assert_eq!(server.logins(), 0);
}

#[test]
fn an_unreachable_host_is_a_network_error() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let remote = server.remote();
    // Nothing listens on the port any more.
    drop(server);

    let error = refusal(Connection::open(&remote, &signer(&card)));

    // Refused, or reset by the closing server.
    assert!(
        matches!(error, RemoteFailure::NoConnection { fallback: None, .. } | RemoteFailure::Unreachable { .. }),
        "{error:?}"
    );
}

/// Wraps a signer to record how much data a login asks it to sign.
struct Measured<'a>(&'a dyn SshSigner, AtomicUsize);

impl SshSigner for Measured<'_> {
    fn public_key(&self) -> &SshPublicKey {
        self.0.public_key()
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, SignFailure> {
        self.1.store(data.len(), Ordering::SeqCst);
        self.0.sign(data)
    }
}

#[test]
fn a_login_for_a_short_user_name_fits_one_short_apdu() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let signer = signer(&card);
    let measured = Measured(&signer, AtomicUsize::new(0));

    Connection::open(&server.remote(), &measured).unwrap();

    // Session id, "git", "ssh-connection", "publickey", the algorithm and
    // the key blob. Under 256 bytes means the card needs neither command
    // chaining nor extended length for it.
    let size = measured.1.load(Ordering::SeqCst);
    println!("login payload: {size} bytes");
    assert!((100..=255).contains(&size), "{size}");
}

/// A clone of a fresh server's store, for the tests that only commit.
fn cloned(server: &GitServer, card: &Arc<SimulatedKey>) -> Phone {
    let dir = Phone::new();
    assert_eq!(steps::clone(&connect(server, card), &server.remote(), &dir.store).unwrap(), Cloned::Store);
    dir
}

/// The reason `commit` gives for refusing `path`.
fn refused_path(dir: &Phone, path: &str) -> String {
    match sync::commit(&dir.store, path, b"x", MESSAGE, &author()) {
        Err(Fault::Local { detail }) => match detail.strip_prefix("not a safe name for an entry: ") {
            Some(why) => why.to_owned(),
            None => panic!("expected {path:?} to be refused as unsafe, got {detail:?}"),
        },
        other => panic!("expected {path:?} to be refused, got {other:?}"),
    }
}

#[test]
fn commit_refuses_names_that_leave_the_store() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = cloned(&server, &card);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);

    for (path, why) in [
        ("../escape.gpg", "`.` or `..`"),
        ("web/../../escape.gpg", "`.` or `..`"),
        ("/tmp/escape.gpg", "absolute"),
        ("", "empty component"),
        ("web//x.gpg", "empty component"),
        ("web/", "empty component"),
        ("web\\x.gpg", "backslash"),
        ("web/x\0.gpg", "NUL"),
    ] {
        let said = refused_path(&dir, path);
        assert!(said.contains(why), "{path:?}: {said}");
    }
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
}

#[test]
fn commit_refuses_gits_own_directory_however_it_is_spelled() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = cloned(&server, &card);
    let config = std::fs::read(dir.path().join(".git/config")).unwrap();

    for path in [
        ".git/config",
        "web/.git/config",
        ".Git/config",
        "web/.GIT/hooks/pre-commit",
        "GIT~1/config",
        ".git./config",
        ".git /config",
        ".git::$INDEX_ALLOCATION/config",
        ".g\u{200c}it/config",
        "web/.git",
    ] {
        let said = refused_path(&dir, path);
        assert!(said.contains("git's own directory"), "{path:?}: {said}");
    }
    // Names that merely resemble it are ordinary entries.
    sync::commit(&dir.store, "web/.github.gpg", b"x", MESSAGE, &author()).unwrap();

    assert_eq!(std::fs::read(dir.path().join(".git/config")).unwrap(), config);
    assert!(!dir.path().join("web/.git").exists());
    assert!(!dir.path().join(".Git").exists());
}

#[test]
fn commit_refuses_to_write_through_a_link_in_the_repository() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("victim.gpg"), "untouched").unwrap();
    // A copy that has links in its history: made before paw refused them,
    // or by another tool.
    let dir = cloned(&server, &card);
    std::os::unix::fs::symlink(outside.path(), dir.path().join("shared")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("victim.gpg"), dir.path().join("victim.gpg")).unwrap();
    git(dir.path(), &["add", "shared", "victim.gpg"]);
    git(dir.path(), &["commit", "--quiet", "-m", "Links."]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);

    let through_folder = refused_path(&dir, "shared/new.gpg");
    let through_file = refused_path(&dir, "victim.gpg");

    assert!(through_folder.contains("one of its folders is a link"), "{through_folder}");
    assert!(through_file.contains("it is a link"), "{through_file}");
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
    assert_eq!(std::fs::read_to_string(outside.path().join("victim.gpg")).unwrap(), "untouched");
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
}

#[test]
fn commit_does_not_follow_a_link_planted_in_the_working_copy() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = cloned(&server, &card);
    let outside = tempfile::tempdir().unwrap();
    // Not in the repository, so only the working copy knows about it.
    std::os::unix::fs::symlink(outside.path(), dir.path().join("web")).unwrap();

    let head = git(dir.path(), &["rev-parse", "HEAD"]);

    let said = refused_path(&dir, ENTRY);

    assert!(said.contains("in the working copy"), "{said}");
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
}

#[test]
fn an_entry_named_like_a_pattern_does_not_touch_other_entries() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = cloned(&server, &card);
    commit_entry(&dir);
    // A change on disk that has not been committed.
    std::fs::write(dir.path().join(ENTRY), "local state").unwrap();

    // `pass` allows these characters in names; to git they are a pattern
    // that also matches the entry above.
    for name in ["web/*", "web/example?com.gpg", "web/[a-z]*.gpg", ":/web/example.com.gpg"] {
        sync::commit(&dir.store, name, b"other", MESSAGE, &author()).unwrap();
        assert_eq!(std::fs::read(dir.path().join(name)).unwrap(), b"other", "{name}");
    }

    assert_eq!(std::fs::read_to_string(dir.path().join(ENTRY)).unwrap(), "local state");
}

/// Bytes that do not compress, so the pack is as big as they are.
fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

#[test]
fn a_download_larger_than_the_limit_is_stopped() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let small = cloned(&server, &card);
    server.sim.commit_file("huge.gpg".into(), noise(3 << 20), "Huge.".into()).unwrap();

    // A clone and a fetch, each with the limit at 1 MiB.
    let connection = connect(&server, &card);
    connection.set_download_limit(1 << 20);
    let dir = Phone::new();
    let cloning = steps::clone(&connection, &server.remote(), &dir.store);
    let fetching = steps::fetch(&connection, &small.store);

    for result in [cloning.map(|_| String::new()), fetching] {
        let Err(SyncError::Fault { fault: Fault::Local { detail: message } }) = result else { panic!("{result:?}") };
        assert!(message.contains("more than 1 MiB"), "{message}");
    }
    assert!(!dir.path().join("huge.gpg").exists());
    assert_ne!(git(small.path(), &["rev-parse", "origin/main"]), server.tip());

    // Under the default limit the same store comes in.
    let connection = connect(&server, &card);
    assert_eq!(steps::fetch(&connection, &small.store).unwrap(), server.tip());
}

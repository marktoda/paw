//! The spike's exit test: clone, commit and push a password store over SSH,
//! with every login signed by a simulated OpenPGP card. The git host is an
//! in-process SSH server in front of the real git programs (see `support`).

mod support;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use paw_core::sim::SimulatedKey;
use paw_core::ssh::{CardSigner, SshPublicKey, SshSigner, card_ssh_public_key};
use paw_core::sync::{self, Author, Connection, HostKeyPin};
use paw_core::{PawError, card_info};
use support::{GitServer, git};

const PIN: &str = "123456";
const ENTRY: &str = "web/example.com.gpg";
const MESSAGE: &str = "Add given password for web/example.com to store.";

/// A card whose authentication slot needs a touch for every signature.
fn card() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), true, 50)
}

fn authorised_line(card: &Arc<SimulatedKey>) -> String {
    card_ssh_public_key(card.clone()).unwrap().authorized_key()
}

/// A signer that touches the card whenever it asks.
fn signer(card: &Arc<SimulatedKey>) -> CardSigner {
    let touch = card.clone();
    CardSigner::new(card.clone(), PIN, move || touch.touch()).unwrap()
}

fn connect(server: &GitServer, card: &Arc<SimulatedKey>) -> Connection {
    Connection::open(&server.remote(), &signer(card)).unwrap()
}

fn author() -> Author {
    Author { name: "iPhone".into(), email: "iphone@example.com".into() }
}

fn commit_entry(dir: &Path) -> String {
    sync::commit(dir, ENTRY, b"ciphertext", MESSAGE, &author()).unwrap()
}

/// `Connection` has no `Debug`, so `unwrap_err` is not available.
fn refusal(result: Result<Connection, PawError>) -> PawError {
    result.err().expect("the connection should have been refused")
}

#[test]
fn clones_commits_and_pushes_with_the_card() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = tempfile::tempdir().unwrap();

    let connection = connect(&server, &card);
    sync::clone(&connection, &server.remote(), dir.path()).unwrap();
    drop(connection);

    assert_eq!(std::fs::read_to_string(dir.path().join("README")).unwrap(), "a password store\n");
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), server.tip());
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), "");
    // A clone is one login, so one touch.
    assert_eq!(card.authentication_count(), 1);

    let commit = commit_entry(dir.path());
    assert_eq!(git(dir.path(), &["log", "-1", "--format=%s|%an|%ae"]), format!("{MESSAGE}|iPhone|iphone@example.com"));
    assert_eq!(git(dir.path(), &["show", "--format=", "--name-only", "HEAD"]), ENTRY);
    assert_ne!(server.tip(), commit);

    let connection = connect(&server, &card);
    sync::push(&connection, dir.path()).unwrap();

    assert_eq!(server.tip(), commit);
    assert_eq!(git(&server.bare(), &["show", &format!("main:{ENTRY}")]), "ciphertext");
    assert_eq!(git(dir.path(), &["rev-parse", "origin/main"]), commit);
    assert_eq!(card.authentication_count(), 2);
}

#[test]
fn fetch_and_push_share_one_login() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = tempfile::tempdir().unwrap();
    sync::clone(&connect(&server, &card), &server.remote(), dir.path()).unwrap();
    let commit = commit_entry(dir.path());
    let touches_before = card.authentication_count();

    let connection = connect(&server, &card);
    let remote_tip = sync::fetch(&connection, dir.path()).unwrap();
    sync::push(&connection, dir.path()).unwrap();

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
    let dir = tempfile::tempdir().unwrap();
    let connection = connect(&server, &card);
    sync::clone(&connection, &server.remote(), dir.path()).unwrap();

    let desktop = server.desktop_commit("desktop.gpg", "from the desktop");
    let remote_tip = sync::fetch(&connection, dir.path()).unwrap();

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
    let dir = tempfile::tempdir().unwrap();
    let connection = connect(&server, &card);
    sync::clone(&connection, &server.remote(), dir.path()).unwrap();

    let desktop = server.desktop_commit("desktop.gpg", "from the desktop");
    commit_entry(dir.path());

    // Before fetching, the remote's new tip is unknown here.
    assert!(matches!(sync::push(&connection, dir.path()), Err(PawError::NotFastForward)));
    // After fetching it is known, and still not an ancestor of ours.
    sync::fetch(&connection, dir.path()).unwrap();
    assert!(matches!(sync::push(&connection, dir.path()), Err(PawError::NotFastForward)));

    assert_eq!(server.tip(), desktop);
}

#[test]
fn a_push_the_host_refuses_is_reported() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = tempfile::tempdir().unwrap();
    let connection = connect(&server, &card);
    sync::clone(&connection, &server.remote(), dir.path()).unwrap();
    let before = server.tip();

    let hook = server.bare().join("hooks/pre-receive");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    commit_entry(dir.path());

    match sync::push(&connection, dir.path()) {
        Err(PawError::Git(why)) => assert!(why.contains("refused the push"), "{why}"),
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

    assert!(matches!(error, PawError::KeyRejected), "{error:?}");
    assert_eq!(error.to_string(), "the git host did not accept this key");
    // No signature, so no touch, and the PIN was never sent.
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
        PawError::HostKeyMismatch { presented } => assert_eq!(presented, real),
        other => panic!("expected HostKeyMismatch, got {other:?}"),
    }
    assert_eq!(card.authentication_count(), 0);
    assert_eq!(server.logins(), 0);
}

#[test]
fn a_wrong_pin_fails_the_login_and_reports_the_tries_left() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let touch = card.clone();
    let signer = CardSigner::new(card.clone(), "000000", move || touch.touch()).unwrap();

    let error = refusal(Connection::open(&server.remote(), &signer));

    assert!(matches!(error, PawError::WrongPin { tries_left: 2 }), "{error:?}");
    assert_eq!(server.logins(), 0);
    assert_eq!(card_info(card).unwrap().pin_tries_left, 2);
}

#[test]
fn an_untouched_key_fails_the_login() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let signer = CardSigner::new(card.clone(), PIN, || {}).unwrap();

    let error = refusal(Connection::open(&server.remote(), &signer));

    assert!(matches!(error, PawError::Sign(_)), "{error:?}");
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

    assert!(matches!(error, PawError::Network(_)), "{error:?}");
}

/// Wraps a signer to record how much data a login asks it to sign.
struct Measured<'a>(&'a CardSigner, AtomicUsize);

impl SshSigner for Measured<'_> {
    fn public_key(&self) -> &SshPublicKey {
        self.0.public_key()
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, PawError> {
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

#[test]
fn commit_refuses_paths_outside_the_store() {
    let card = card();
    let server = GitServer::start(&[authorised_line(&card)]);
    let dir = tempfile::tempdir().unwrap();
    sync::clone(&connect(&server, &card), &server.remote(), dir.path()).unwrap();

    for path in ["../escape.gpg", "/tmp/escape.gpg", ".git/config", ""] {
        let result = sync::commit(dir.path(), path, b"x", MESSAGE, &author());
        assert!(matches!(result, Err(PawError::Git(_))), "{path}");
    }
}

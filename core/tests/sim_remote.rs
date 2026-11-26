//! The simulated git host on a chosen port, and serving the same
//! repository again after the app is launched again.

#![cfg(feature = "sim")]

mod key_common;

use std::sync::Arc;

use key_common::{PIN, signer};
use paw_core::sim::remote::steps;
use paw_core::sim::SimulatedKey;
use paw_core::sim::remote::SimRemote;
use paw_core::ffi::sync::Setup;
use paw_core::files::layout::store_repo_dir;
use paw_core::setup::SetupState;
use paw_core::sync::Connection;

fn key() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), false, 1000)
}

/// Starts a host with `start` on a port nothing listened on a moment
/// before, and that port. A port another test took meanwhile is given up
/// for another.
fn on_a_free_port(start: impl Fn(u16) -> Result<Arc<SimRemote>, paw_core::PawError>) -> (u16, Arc<SimRemote>) {
    let mut last = None;
    for _ in 0..5 {
        let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        match start(port) {
            Ok(host) => return (port, host),
            Err(e) => last = Some(e),
        }
    }
    panic!("no free port to listen on: {last:?}");
}

/// Clones what `host` serves and returns the checked-out tip.
fn clone_tip(host: &SimRemote, key: &Arc<SimulatedKey>) -> String {
    let signer = signer(key);
    let remote = host.remote().unwrap();
    let connection = Connection::open(&remote, &signer).unwrap();
    let dir = tempfile::tempdir().unwrap();
    steps::clone(&connection, &remote, &dir.path().join("repo")).unwrap();
    git2::Repository::open(dir.path().join("repo")).unwrap().head().unwrap().target().unwrap().to_string()
}

#[test]
fn a_host_started_on_a_chosen_port_listens_there() {
    let key = key();
    let (port, host) = on_a_free_port(|port| SimRemote::start_on(key.clone(), port));

    assert_eq!(host.port(), port);
    assert_eq!(host.address(), format!("ssh://git@127.0.0.1:{port}/store.git"));
    assert_eq!(Some(clone_tip(&host, &key)), host.tip());
    // Port 0 still means any free port.
    assert_ne!(SimRemote::start_on(key, 0).unwrap().port(), 0);
}

#[test]
fn a_relaunched_host_serves_the_same_repository_at_the_same_address() {
    let key = key();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("remote").to_string_lossy().into_owned();

    let (port, first) = on_a_free_port(|port| SimRemote::serve(key.clone(), folder.clone(), port));
    let address = first.address();
    let fingerprint = first.host_key_fingerprint();
    let changed = first.commit_file("added.gpg".into(), b"x".to_vec(), "Another machine.".into()).unwrap();
    assert_eq!(clone_tip(&first, &key), changed);
    drop(first);
    // The repository outlives the host that served it.
    assert!(dir.path().join("remote/store.git").is_dir());

    let second = SimRemote::serve(key.clone(), folder, port).unwrap();

    assert_eq!(second.address(), address);
    assert_eq!(second.host_key_fingerprint(), fingerprint);
    assert_eq!(second.tip(), Some(changed.clone()));
    assert_eq!(clone_tip(&second, &key), changed);
}

#[test]
fn a_host_started_without_a_folder_removes_its_repository() {
    let host = SimRemote::start_on(key(), 0).unwrap();
    let bare = host.bare();
    assert!(bare.is_dir());
    drop(host);
    assert!(!bare.exists());
}

#[test]
fn a_store_set_up_from_the_host_is_done_and_at_its_tip() {
    let key = key();
    let host = SimRemote::start(key.clone()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().to_string_lossy().into_owned();

    host.set_up_store(key.clone(), PIN.into(), state_dir.clone()).unwrap();

    assert!(matches!(Setup::open(state_dir.clone()).unwrap().state(), SetupState::Done { .. }));
    let repo = git2::Repository::open(store_repo_dir(state_dir)).unwrap();
    assert_eq!(repo.head().unwrap().target().map(|id| id.to_string()), host.tip());
    assert_eq!(key.pin_tries_left(), 3);
}

#[test]
fn a_set_up_store_downloads_the_list_of_keys_another_computer_gave_work() {
    for missing_key in [false, true] {
        let key = key();
        let host = SimRemote::start(key.clone()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().to_string_lossy().into_owned();
        host.set_up_store(key.clone(), PIN.into(), state_dir.clone()).unwrap();

        host.give_work_its_own_keys(key.clone(), PIN.into(), state_dir.clone(), missing_key).unwrap();

        let repo = std::path::PathBuf::from(store_repo_dir(state_dir));
        let lines = std::fs::read_to_string(repo.join("work/.gpg-id")).unwrap();
        let mut expected = vec![key.fingerprint_hex()];
        if missing_key {
            expected.push(SimulatedKey::new_other(PIN.into(), false, 1000).fingerprint_hex());
        }
        assert_eq!(lines.lines().collect::<Vec<_>>(), expected);
        let head = git2::Repository::open(&repo).unwrap().head().unwrap().target().map(|id| id.to_string());
        assert_eq!(head, host.tip());
    }
}

#[test]
fn setting_up_with_the_wrong_pin_fails_without_locking_the_key() {
    let key = key();
    let host = SimRemote::start(key.clone()).unwrap();
    let dir = tempfile::tempdir().unwrap();

    let failed = host.set_up_store(key.clone(), "654321".into(), dir.path().to_string_lossy().into_owned());

    assert!(failed.is_err());
    assert_eq!(key.pin_tries_left(), 2);
}

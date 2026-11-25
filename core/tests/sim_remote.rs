//! The simulated git host on a chosen port, and serving the same
//! repository again after the app is launched again.

#![cfg(feature = "sim")]

mod key_common;

use std::sync::Arc;

use key_common::{PIN, signer};
use paw_core::sim::remote::steps;
use paw_core::sim::SimulatedKey;
use paw_core::sim::remote::SimRemote;
use paw_core::sync::Connection;

fn key() -> Arc<SimulatedKey> {
    SimulatedKey::new(PIN.into(), false, 1000)
}

/// A port nothing listens on right now.
fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
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
    let port = free_port();

    let host = SimRemote::start_on(key.clone(), port).unwrap();

    assert_eq!(host.port(), port);
    assert_eq!(host.address(), format!("ssh://git@127.0.0.1:{port}/store.git"));
    assert_eq!(Some(clone_tip(&host, &key)), host.tip());
    // Port 0 still means any free port.
    assert_ne!(SimRemote::start_on(key, 0).unwrap().port(), 0);
}

#[test]
fn a_relaunched_host_serves_the_same_repository_at_the_same_address() {
    let key = key();
    let port = free_port();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("remote").to_string_lossy().into_owned();

    let first = SimRemote::serve(key.clone(), folder.clone(), port).unwrap();
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

//! When github.com cannot be reached on port 22, setup offers GitHub's
//! port-443 endpoint, and uses it only when asked. Which failures offer it
//! is a unit test of `sync::transport`; here, setup's side of it, and the
//! connection failures that are not a blocked port, against this machine.

#![cfg(feature = "sim")]

mod key_common;

use std::net::TcpListener;
use std::sync::Arc;

use key_common::{WAIT, port_with};
use paw_core::fault::Fault;
use paw_core::setup::Setup;
use paw_core::key::KeyPort;
use paw_core::remote::{PortFallback, RemoteFailure};
use paw_core::setup::{SetupError, SetupState};
use paw_core::sim::SimTouch;
use paw_core::sim::remote::SimRemote;
use paw_core::ssh::SshPublicKey;
use paw_core::sync::address::{AddressNotice, StoreAddress};
use paw_core::sync::hosts::{self, KnownHosts};
use paw_core::sync::{Connection, Remote};

/// A port nothing listens on, held (bound, not listening) until it is
/// dropped so that no other test can listen there meanwhile.
fn closed_port() -> (u16, tokio::net::TcpSocket) {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind(([127, 0, 0, 1], 0).into()).unwrap();
    (socket.local_addr().unwrap().port(), socket)
}

fn at_check_host(dir: &std::path::Path, port: &Arc<KeyPort>, address: &str) -> Arc<Setup> {
    let setup = Setup::open(dir.to_string_lossy().into_owned()).unwrap();
    setup.read_key_from_session(port.session().unwrap()).unwrap();
    setup.set_address(address.into()).unwrap();
    setup
}

fn key_port() -> Arc<KeyPort> {
    port_with(&key_common::sim(SimTouch::Off, SimTouch::Off, WAIT))
}

#[test]
fn port_443_is_used_only_when_asked_and_kept_with_its_notice() {
    let port = key_port();
    let dir = tempfile::tempdir().unwrap();
    let setup = at_check_host(dir.path(), &port, "you/store");
    assert!(matches!(setup.state(), SetupState::CheckHost { address, .. } if address == "git@github.com:you/store.git"));

    let state = setup.use_port_fallback().unwrap();

    let SetupState::CheckHost { address, notice } = state else { panic!("{state:?}") };
    assert_eq!(address, "ssh://git@ssh.github.com:443/you/store.git");
    let fallback = PortFallback { from_host: "github.com".into(), from_port: 22, host: "ssh.github.com".into(), port: 443 };
    assert_eq!(notice, Some(AddressNotice::PortFallback { fallback }));
    // Survives a relaunch, as any address does.
    let reopened = Setup::open(dir.path().to_string_lossy().into_owned()).unwrap();
    assert!(matches!(reopened.state(), SetupState::CheckHost { address, notice: Some(_) } if address.contains(":443/")));
    // Port 443 has no other way in.
    assert!(setup.use_port_fallback().is_err());
}

/// The remote setup builds for the port-443 address, sent to a simulated
/// host instead of ssh.github.com (the one change: where it dials), is
/// refused for presenting a key that is not GitHub's.
#[test]
fn port_443_refuses_a_host_that_presents_another_key() {
    let port = key_port();
    let dir = tempfile::tempdir().unwrap();
    let setup = at_check_host(dir.path(), &port, "you/store");
    let SetupState::CheckHost { address, .. } = setup.use_port_fallback().unwrap() else { panic!() };
    let address = StoreAddress::parse(&address).unwrap().address;
    let remote = KnownHosts::of(dir.path()).remote(&address).unwrap().expect("held to GitHub's keys with no confirmation");
    let key = key_common::sim(SimTouch::Off, SimTouch::Off, WAIT);
    let impostor = SimRemote::start(key.clone()).unwrap();
    let dialled = Remote { host: "127.0.0.1".into(), port: impostor.port(), ..remote };

    let error = Connection::probe(&dialled, &SshPublicKey::ed25519(&key.authentication_public_key())).unwrap_err();

    let Fault::Remote { failure: RemoteFailure::HostKeyChanged { expected, presented, .. } } = error else { panic!("{error:?}") };
    assert_eq!(expected, hosts::bundled("github.com", 22).unwrap().fingerprints());
    assert_eq!(presented, impostor.host_key_fingerprint());
    assert_eq!(impostor.logins(), 0);
}

#[test]
fn a_host_that_is_not_github_offers_nothing() {
    let port = key_port();
    let dir = tempfile::tempdir().unwrap();
    let (closed, _held) = closed_port();
    let setup = at_check_host(dir.path(), &port, &format!("ssh://git@127.0.0.1:{closed}/store.git"));

    let error = setup.check_host().unwrap_err();

    let SetupError::Fault { fault: Fault::Remote { failure: RemoteFailure::NoConnection { fallback, .. } } } = &error else {
        panic!("{error:?}")
    };
    assert_eq!(*fallback, None);
    assert!(setup.use_port_fallback().is_err());
}

#[test]
fn a_host_that_hangs_up_before_ssh_is_not_a_blocked_port() {
    let hangs_up = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let hang_up_port = hangs_up.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in hangs_up.incoming() {
            drop(stream);
        }
    });
    let port = key_port();
    let dir = tempfile::tempdir().unwrap();
    let setup = at_check_host(dir.path(), &port, &format!("ssh://git@127.0.0.1:{hang_up_port}/store.git"));

    let error = setup.check_host().unwrap_err();

    assert!(matches!(error, SetupError::Fault { fault: Fault::Remote { failure: RemoteFailure::Unreachable { .. } } }), "{error:?}");
}

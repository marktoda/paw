//! When github.com cannot be reached on port 22, setup offers GitHub's
//! port-443 endpoint, and uses it only when asked. github.com is played by
//! a closed port and by the simulated host, through setup's routes.

#![cfg(feature = "sim")]

mod key_common;

use std::net::TcpListener;
use std::sync::Arc;

use key_common::{WAIT, port_with};
use paw_core::ffi::sync::Setup;
use paw_core::key::KeyPort;
use paw_core::remote::RemoteFailure;
use paw_core::setup::{SetupError, SetupState};
use paw_core::sim::SimTouch;
use paw_core::sim::remote::{SimRemote, SimRoutes};
use paw_core::sync::address::AddressNotice;
use paw_core::remote::PortFallback;

/// A port nothing listens on.
fn closed_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

/// The other way in a failed host check offered, if it made no connection.
fn offered(error: &SetupError) -> Option<PortFallback> {
    match error {
        SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::NoConnection { fallback, .. } } } => fallback.clone(),
        _ => None,
    }
}

fn at_check_host(routes: &SimRoutes, dir: &std::path::Path, port: &Arc<KeyPort>, address: &str) -> Arc<Setup> {
    let setup = Setup::open_routed(dir.to_string_lossy().into_owned(), routes).unwrap();
    setup.read_key_from_session(port.session().unwrap()).unwrap();
    setup.set_address(address.into()).unwrap();
    setup
}

#[test]
fn port_443_is_offered_only_when_port_22_cannot_be_reached_and_used_only_when_asked() {
    let key = key_common::sim(SimTouch::Off, SimTouch::Off, WAIT);
    let port = port_with(&key);
    let host = SimRemote::start(key.clone()).unwrap();
    let routes = SimRoutes::new();

    // Port 22 refuses connections.
    routes.redirect("github.com", 22, Some(closed_port()));
    let dir = tempfile::tempdir().unwrap();
    let setup = at_check_host(&routes, dir.path(), &port, "you/store");

    let error = setup.check_host().unwrap_err();

    assert!(matches!(&error, SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::NoConnection { host, .. } } } if host == "github.com"), "{error:?}");
    assert_eq!(
        offered(&error),
        Some(PortFallback { from_host: "github.com".into(), from_port: 22, host: "ssh.github.com".into(), port: 443 })
    );
    // Offered, not used.
    assert!(matches!(setup.state(), SetupState::CheckHost { address, .. } if address == "git@github.com:you/store.git"));

    let state = setup.use_port_fallback().unwrap();

    let SetupState::CheckHost { address, notice } = state else { panic!("{state:?}") };
    assert_eq!(address, "ssh://git@ssh.github.com:443/you/store.git");
    let fallback = AddressNotice::PortFallback { fallback: PortFallback { from_host: "github.com".into(), from_port: 22, host: "ssh.github.com".into(), port: 443 } };
    assert_eq!(notice, Some(fallback));
    // Survives a relaunch, as any address does.
    assert!(matches!(Setup::open(dir.path().to_string_lossy().into_owned()).unwrap().state(), SetupState::CheckHost { address, .. } if address.contains(":443/")));

    // ssh.github.com:443 is held to GitHub's bundled keys: a host with any
    // other key (here, the simulated one) is refused, with no confirmation
    // offered and nothing sent.
    routes.redirect("ssh.github.com", 443, Some(host.port()));
    let error = setup.check_host().unwrap_err();
    assert!(
        matches!(&error, SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::HostKeyChanged { host, bundled: true, .. } } } if host == "ssh.github.com"),
        "{error:?}"
    );
    assert_eq!(host.logins(), 0);
    assert!(setup.use_port_fallback().is_err());

    // A host that answers on port 22 with another key is not a blocked
    // port: no offer.
    routes.redirect("github.com", 22, Some(host.port()));
    let dir = tempfile::tempdir().unwrap();
    let setup = at_check_host(&routes, dir.path(), &port, "git@github.com:you/store.git");
    let error = setup.check_host().unwrap_err();
    assert!(matches!(error, SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::HostKeyChanged { .. } } }), "{error:?}");

    // Nor is one that accepts the connection and hangs up before SSH.
    let hangs_up = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let hang_up_port = hangs_up.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in hangs_up.incoming() {
            drop(stream);
        }
    });
    routes.redirect("github.com", 22, Some(hang_up_port));
    let error = setup.check_host().unwrap_err();
    assert!(matches!(error, SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::Unreachable { .. } } }), "{error:?}");

    // Another host's blocked port has nothing to offer.
    let dir = tempfile::tempdir().unwrap();
    let setup = at_check_host(&routes, dir.path(), &port, &format!("ssh://git@127.0.0.1:{}/store.git", closed_port()));
    let error = setup.check_host().unwrap_err();
    assert!(matches!(error, SetupError::Fault { fault: paw_core::fault::Fault::Remote { failure: RemoteFailure::NoConnection { .. } } }), "{error:?}");
    assert_eq!(offered(&error), None);
    assert!(setup.use_port_fallback().is_err());
}

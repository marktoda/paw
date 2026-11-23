//! When github.com cannot be reached on port 22, setup offers GitHub's
//! port-443 endpoint, and uses it only when asked. github.com is played by
//! a closed port and by the simulated host, through a process-wide
//! redirect, so this file is its own test binary and runs as one test.

#![cfg(feature = "sim")]

use std::net::TcpListener;
use std::sync::Arc;

use paw_core::setup::outcome::RemoteReason;
use paw_core::setup::{PortFallback, SetupError, SetupFlow, SetupState};
use paw_core::sim::SimulatedKey;
use paw_core::sim::remote::{SimRemote, redirect_connections};

const PIN: &str = "123456";

/// A port nothing listens on.
fn closed_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

fn at_check_host(dir: &std::path::Path, key: &Arc<SimulatedKey>, address: &str) -> SetupFlow {
    let mut flow = SetupFlow::open(dir).unwrap();
    flow.read_key(key.clone()).unwrap();
    flow.set_address(address).unwrap();
    flow
}

#[test]
fn port_443_is_offered_only_when_port_22_cannot_be_reached_and_used_only_when_asked() {
    let key = SimulatedKey::new(PIN.into(), false, 1000);
    let host = SimRemote::start(key.clone()).unwrap();

    // Port 22 refuses connections.
    redirect_connections("github.com", 22, Some(closed_port()));
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_check_host(dir.path(), &key, "you/store");

    let error = flow.check_host().unwrap_err();

    assert!(matches!(&error, SetupError::HostUnreachable { host, .. } if host == "github.com"), "{error:?}");
    assert_eq!(flow.last_remote_message().unwrap().reason, RemoteReason::Unreachable);
    assert_eq!(
        flow.port_fallback(),
        Some(PortFallback {
            from_host: "github.com".into(),
            from_port: 22,
            host: "ssh.github.com".into(),
            port: 443,
            address: "ssh://git@ssh.github.com:443/you/store.git".into(),
        })
    );
    // Offered, not used.
    assert!(matches!(flow.state(), SetupState::CheckHost { address, .. } if address == "git@github.com:you/store.git"));

    let state = flow.use_port_fallback().unwrap();

    let SetupState::CheckHost { address, notice } = state else { panic!("{state:?}") };
    assert_eq!(address, "ssh://git@ssh.github.com:443/you/store.git");
    assert!(notice.unwrap().contains("port 443"));
    assert_eq!(flow.port_fallback(), None);
    // Survives a relaunch, as any address does.
    assert!(matches!(SetupFlow::open(dir.path()).unwrap().state(), SetupState::CheckHost { address, .. } if address.contains(":443/")));

    // ssh.github.com:443 is held to GitHub's bundled keys: a host with any
    // other key (here, the simulated one) is refused, with no confirmation
    // offered and nothing sent.
    redirect_connections("ssh.github.com", 443, Some(host.port()));
    let error = flow.check_host().unwrap_err();
    assert!(matches!(&error, SetupError::HostKeyChanged { host, .. } if host == "ssh.github.com"), "{error:?}");
    assert_eq!(flow.port_fallback(), None);
    assert_eq!(host.logins(), 0);
    assert!(flow.use_port_fallback().is_err());

    // A host that answers on port 22 with another key is not a blocked
    // port: no offer.
    redirect_connections("github.com", 22, Some(host.port()));
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_check_host(dir.path(), &key, "git@github.com:you/store.git");
    assert!(matches!(flow.check_host().unwrap_err(), SetupError::HostKeyChanged { .. }));
    assert_eq!(flow.port_fallback(), None);

    // Nor is one that accepts the connection and hangs up before SSH.
    let hangs_up = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let hang_up_port = hangs_up.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in hangs_up.incoming() {
            drop(stream);
        }
    });
    redirect_connections("github.com", 22, Some(hang_up_port));
    assert!(matches!(flow.check_host().unwrap_err(), SetupError::HostUnreachable { .. }));
    assert_eq!(flow.port_fallback(), None);

    // Another host's blocked port has nothing to offer.
    let dir = tempfile::tempdir().unwrap();
    let mut flow = at_check_host(dir.path(), &key, &format!("ssh://git@127.0.0.1:{}/store.git", closed_port()));
    assert!(matches!(flow.check_host().unwrap_err(), SetupError::HostUnreachable { .. }));
    assert_eq!(flow.port_fallback(), None);

    redirect_connections("github.com", 22, None);
    redirect_connections("ssh.github.com", 443, None);
}

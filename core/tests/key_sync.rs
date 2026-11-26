//! A clone over SSH and an entry open through one key session: the PIN is
//! typed once and serves both.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use key_common::*;
use paw_core::sim::remote::steps;
use paw_core::key::LoginPurpose;
use paw_core::task::{KeyOutcome, KeyTaskState};
use paw_core::sim::SimTouch;
use paw_core::sync::Connection;
use support::{GitServer, git};

#[test]
fn a_clone_and_an_entry_open_take_one_pin_entry() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    let server = GitServer::start(&[rig.sim.ssh_public_key()]);
    let dir = tempfile::tempdir().unwrap();

    let driver = Driver::helpful(PIN, &rig.sim);
    let login = paw_core::sim::tasks::SimLogin::start(&rig.port, LoginPurpose::Clone, driver.clone());
    assert!(is_pin(&login.task().state()), "the one PIN stage");
    driver.drive(login.task());
    // The PIN is verified before the connection opens.
    let signer = login.signer().unwrap();
    assert_eq!(rig.sim.verify_attempts(), 1);
    let connection = Connection::open(&server.remote(), &signer).unwrap();
    steps::clone(&connection, &server.remote(), dir.path()).unwrap();
    drop(connection);
    login.finish();
    assert!(matches!(finish(login.task()), KeyTaskState::Done { outcome: KeyOutcome::SignedIn }));
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), server.tip());

    let opening = Driver::helpful(PIN, &rig.sim);
    let entry = open(&rig, ENTRY, &opening);
    assert!(matches!(finish(&entry), KeyTaskState::Done { .. }));

    assert!(!opening.saw(is_pin), "no PIN stage for the open");
    assert_eq!(rig.sim.verify_attempts(), 1);
    assert_eq!(rig.sim.authentication_count(), 1);
    assert_eq!(rig.sim.decipher_count(), 1);
}

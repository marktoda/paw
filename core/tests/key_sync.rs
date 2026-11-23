//! A clone over SSH and an entry open through one key session: the PIN is
//! typed once and serves both.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use key_common::*;
use paw_core::key::{KeyOutcome, KeyTaskState, LoginPurpose};
use paw_core::sim::SimTouch;
use paw_core::ssh::card_ssh_public_key;
use paw_core::sync::{self, Connection};
use support::{GitServer, git};

#[test]
fn a_clone_and_an_entry_open_take_one_pin_entry() {
    let rig = rig(SimTouch::EachUse, SimTouch::EachUse);
    let line = card_ssh_public_key(rig.sim.clone()).unwrap().authorized_key();
    let server = GitServer::start(&[line]);
    let dir = tempfile::tempdir().unwrap();

    let login = Driver::helpful(PIN, &rig.sim);
    let task = rig.port.clone().sign_in(LoginPurpose::Clone, login.clone());
    assert!(is_pin(&task.state()), "the one PIN stage");
    login.drive(&task);
    // The PIN is verified before the connection opens.
    let signer = task.login_signer().unwrap();
    assert_eq!(rig.sim.verify_attempts(), 1);
    let connection = Connection::open(&server.remote(), &signer).unwrap();
    sync::clone(&connection, &server.remote(), dir.path()).unwrap();
    drop(connection);
    task.finish_login();
    assert!(matches!(finish(&task), KeyTaskState::Done { outcome: KeyOutcome::SignedIn }));
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), server.tip());

    let opening = Driver::helpful(PIN, &rig.sim);
    let entry = open(&rig, ENTRY, &opening);
    assert!(matches!(finish(&entry), KeyTaskState::Done { .. }));

    assert!(!opening.saw(is_pin), "no PIN stage for the open");
    assert_eq!(rig.sim.verify_attempts(), 1);
    assert_eq!(rig.sim.authentication_count(), 1);
    assert_eq!(rig.sim.decipher_count(), 1);
}

//! The device checks, run against the simulated card so their commands are
//! known to be well formed before they meet a real key.

#![cfg(feature = "sim")]

use std::sync::Arc;
use std::time::Duration;

use paw_core::card_info;
use paw_core::diag::{card_checks, card_touch_timeout_check};
use paw_core::sim::{SimOptions, SimulatedKey};

const PIN: &str = "123456";

fn key() -> Arc<SimulatedKey> {
    SimulatedKey::with_options(SimOptions {
        pin: PIN.into(),
        decrypt_touch: true,
        auth_touch: true,
        touch_timeout: Duration::from_millis(50),
        extended_length: true,
    })
}

fn status<'a>(results: &'a [paw_core::diag::CheckResult], name: &str) -> &'a str {
    &results.iter().find(|r| r.name == name).unwrap_or_else(|| panic!("no check named {name}")).status
}

#[test]
fn every_check_succeeds_on_a_touched_key() {
    let key = key();
    let results = card_checks(key.clone(), PIN, &|| key.touch());

    assert_eq!(status(&results, "Ask whether the PIN is verified, before"), "63C3");
    assert_eq!(status(&results, "Ask whether the PIN is verified, after"), "9000");
    for r in &results {
        if r.name != "Ask whether the PIN is verified, before" {
            assert_eq!(r.status, "9000", "{}", r.name);
        }
    }
    for name in [
        "Sign a login, short length (touch)",
        "Sign a login, extended length (touch)",
        "Sign a login, chained, last part (touch)",
    ] {
        let r = results.iter().find(|r| r.name == name).unwrap();
        assert_eq!(r.response_len, 64, "{name}");
    }
    assert_eq!(key.authentication_count(), 3);
}

#[test]
fn results_never_contain_the_pin() {
    let key = key();
    let results = card_checks(key.clone(), PIN, &|| key.touch());
    assert!(!format!("{results:?}").contains(PIN));
    assert!(!format!("{results:?}").contains("31 32 33 34 35 36"));
}

#[test]
fn a_wrong_pin_stops_after_one_try() {
    let key = key();
    let results = card_checks(key.clone(), "000000", &|| key.touch());

    assert_eq!(results.last().unwrap().name, "Verify the PIN, short length");
    assert_eq!(results.last().unwrap().status, "6982");
    assert_eq!(card_info(key.clone()).unwrap().pin_tries_left, 2);
    assert_eq!(key.authentication_count(), 0);
}

#[test]
fn the_timeout_check_reports_an_untouched_key() {
    let key = key();
    let results = card_touch_timeout_check(key.clone(), PIN);

    assert_eq!(status(&results, "Sign a login with no touch"), "6982");
    assert_eq!(key.authentication_count(), 0);
}

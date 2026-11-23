//! Built and run with and without the `sim` feature: every other test file
//! needs the simulation, so `cargo test --no-default-features` would
//! otherwise compile the library's tests and nothing that uses it from
//! outside. Run it as `cargo test --no-default-features`.

#[test]
fn the_device_build_keeps_its_limits() {
    assert_eq!(paw_core::MAX_DECIPHERS, 1);
    assert_eq!(paw_core::sync::DOWNLOAD_LIMIT, 256 << 20);
    let refused = paw_core::sync::RefusedPath { path: "up".into(), reason: "it is a symbolic link".into() };
    assert_eq!(refused.clone(), refused);
}

#[test]
fn the_device_build_talks_to_github_only_as_paws_app() {
    let endpoints = paw_core::github::Endpoints::github();
    assert_eq!(endpoints.web(), "https://github.com");
    assert_eq!(endpoints.api(), "https://api.github.com");
    assert_eq!(endpoints.client_id(), "Iv23liyBePbnvQPCcGRz");
    assert_eq!(paw_core::github::CLIENT_ID, "Iv23liyBePbnvQPCcGRz");
    assert_eq!(endpoints.install_url(), "https://github.com/apps/paw-password-store/installations/new");
    // Without the `sim` feature there is no other constructor: this is the
    // only way to make `Endpoints` (checked by compiling this file with
    // `--no-default-features`, where `Endpoints::simulated` does not exist).
    assert_eq!(endpoints, paw_core::github::Endpoints::github());
}

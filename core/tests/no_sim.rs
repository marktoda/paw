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

# paw

A [pass](https://www.passwordstore.org/) client for iPhone and iPad that
decrypts with a YubiKey over USB-C. Early work: nothing here is usable yet.

- `core/`: Rust. Parses entries and drives the OpenPGP card; the private key
  never leaves the YubiKey. `cargo test` runs it against a simulated card.
- `app/`: SwiftUI app and AutoFill extension. The Xcode project is generated
  from `app/project.yml` with XcodeGen.
- `scripts/build-core.sh`: builds the core for iOS and generates the Swift
  bindings (macOS only).

`nix develop` provides the Rust toolchain.

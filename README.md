# paw

A [pass](https://www.passwordstore.org/) client for iPhone and iPad that
decrypts with a YubiKey over USB-C. Early work: it runs on the owner's
iPhone; [docs/roadmap.md](docs/roadmap.md) says what works and what is left.

- `core/`: Rust. Reads and writes the store, drives the OpenPGP card, syncs
  with git over SSH and keeps Face ID copies; the private key never leaves
  the YubiKey. `cargo test` runs it against a simulated card and git host.
- `app/`: SwiftUI app and AutoFill extension. The Xcode project is generated
  from `app/project.yml` with XcodeGen.
- `scripts/build-core.sh`: builds the core for iOS and generates the Swift
  bindings; on Linux, or with `--bindings-only`, the bindings only.

`nix develop` provides the Rust toolchain and what `scripts/install.sh`
needs to sign and install a build from Linux.

## Licence

MIT. See [LICENSE](LICENSE).

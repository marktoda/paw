#!/usr/bin/env bash
# Builds the Rust core for iPhone and the simulator and generates the Swift
# bindings. Pure Rust, so it runs on Linux as well as macOS.
#
# Output, all under app/:
#   build/core/ios/libpaw_core.a       device library
#   build/core/ios-sim/libpaw_core.a   simulator library
#   build/core/headers/                C header and module map
#   Generated/paw_core.swift           Swift bindings
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/app/build/core"
gen="$root/app/Generated"
rm -rf "$out" "$gen"
mkdir -p "$out/ios" "$out/ios-sim" "$out/headers" "$out/bindings" "$gen"

cd "$root/core"
cargo build --lib --release --target aarch64-apple-ios
cargo build --lib --release --target aarch64-apple-ios-sim
cp target/aarch64-apple-ios/release/libpaw_core.a "$out/ios/"
cp target/aarch64-apple-ios-sim/release/libpaw_core.a "$out/ios-sim/"

# The bindings are read out of a host build of the same library.
cargo build --lib
cargo run --bin uniffi-bindgen -- generate \
  --library target/debug/libpaw_core.a --language swift --out-dir "$out/bindings"

mv "$out/bindings/paw_core.swift" "$gen/"
mv "$out/bindings/paw_coreFFI.h" "$out/headers/"
mv "$out/bindings/paw_coreFFI.modulemap" "$out/headers/module.modulemap"
rmdir "$out/bindings"

#!/usr/bin/env bash
# Builds the Rust core for iPhone and the simulator, generates the Swift
# bindings, and packs both into app/build/PawCore.xcframework.
# Runs on macOS (needs xcodebuild).
set -euo pipefail
cd "$(dirname "$0")/.."

out=app/build
gen=app/Generated
rm -rf "$out" "$gen"
mkdir -p "$out/headers" "$gen"

for target in aarch64-apple-ios aarch64-apple-ios-sim; do
  cargo build --manifest-path core/Cargo.toml --lib --release --target "$target"
done

# The bindings are read out of a host build of the same library.
cargo build --manifest-path core/Cargo.toml
cargo run --manifest-path core/Cargo.toml --bin uniffi-bindgen -- \
  generate --library core/target/debug/libpaw_core.a \
  --language swift --out-dir "$out/bindings"

mv "$out/bindings/paw_core.swift" "$gen/"
mv "$out/bindings/paw_coreFFI.h" "$out/headers/"
mv "$out/bindings/paw_coreFFI.modulemap" "$out/headers/module.modulemap"

xcodebuild -create-xcframework \
  -library core/target/aarch64-apple-ios/release/libpaw_core.a -headers "$out/headers" \
  -library core/target/aarch64-apple-ios-sim/release/libpaw_core.a -headers "$out/headers" \
  -output "$out/PawCore.xcframework"

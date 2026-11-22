#!/usr/bin/env bash
# Builds the Rust core for iPhone and the simulator and generates the Swift
# bindings.
#
# The core contains C: libgit2 (vendored by libgit2-sys) and ring's
# primitives. Compiling those for iOS needs the Apple SDK, so the iOS
# libraries are built on macOS only. On Linux this script still makes the
# host build and the Swift bindings, which is enough to check that the
# interface Swift sees is intact; pass --bindings-only to do the same on a
# Mac.
#
# Output, all under app/:
#   build/core/ios/libpaw_core.a       device library      (macOS only)
#   build/core/ios-sim/libpaw_core.a   simulator library   (macOS only)
#   build/core/headers/                C header and module map
#   Generated/paw_core.swift           Swift bindings
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/app/build/core"
gen="$root/app/Generated"

ios=yes
if [[ "${1:-}" == "--bindings-only" ]]; then
  ios=no
elif [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-core: not on macOS, so no iOS libraries; generating the bindings only" >&2
  ios=no
fi

rm -rf "$out" "$gen"
mkdir -p "$out/headers" "$out/bindings" "$gen"

cd "$root/core"
if [[ "$ios" == yes ]]; then
  mkdir -p "$out/ios" "$out/ios-sim"
  # The C parts (libgit2, ring) otherwise target the newest SDK, not the
  # app's deployment target in app/project.yml.
  export IPHONEOS_DEPLOYMENT_TARGET=26.0
  cargo build --lib --release --target aarch64-apple-ios
  cargo build --lib --release --target aarch64-apple-ios-sim
  cp target/aarch64-apple-ios/release/libpaw_core.a "$out/ios/"
  cp target/aarch64-apple-ios-sim/release/libpaw_core.a "$out/ios-sim/"
fi

# The bindings are read out of a host build of the same library.
cargo build --lib
cargo run --bin uniffi-bindgen -- generate \
  --library target/debug/libpaw_core.a --language swift --out-dir "$out/bindings"

mv "$out/bindings/paw_core.swift" "$gen/"
mv "$out/bindings/paw_coreFFI.h" "$out/headers/"
mv "$out/bindings/paw_coreFFI.modulemap" "$out/headers/module.modulemap"
rmdir "$out/bindings"

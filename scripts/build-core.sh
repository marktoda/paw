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
#
# All of these are built with the default `sim` feature: the simulated key,
# keystore, clock, fixture and git host are in them, so the simulator and
# Debug builds on a device can use them.
#
# --release-device also builds what a future Release configuration needs,
# without the `sim` feature, and proves it with scripts/check-no-sim.sh:
#   build/core/release/ios/libpaw_core.a   device library      (macOS only)
#   build/core/release/headers/            its C header and module map
#   build/core/release/paw_core.swift      its Swift bindings
# The Release bindings must come from the same features as the library:
# UniFFI's Swift checks every exported function's checksum when it loads,
# so the default bindings cannot be linked against a library without the
# simulation symbols. Swift that names a Sim* type has to be left out of
# the Release build (`#if DEBUG`) for it to compile against these.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/app/build/core"
gen="$root/app/Generated"

ios=yes
release=no
for arg in "$@"; do
  case "$arg" in
    --bindings-only) ios=no ;;
    --release-device) release=yes ;;
    *) echo "build-core: unknown option $arg" >&2; exit 2 ;;
  esac
done
if [[ "$ios" == yes && "$(uname -s)" != "Darwin" ]]; then
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

if [[ "$release" == yes ]]; then
  rel="$out/release"
  mkdir -p "$rel/headers" "$rel/bindings"
  if [[ "$ios" == yes ]]; then
    mkdir -p "$rel/ios"
    cargo build --lib --release --no-default-features --target aarch64-apple-ios \
      --target-dir target/no-sim
    cp target/no-sim/aarch64-apple-ios/release/libpaw_core.a "$rel/ios/"
    "$root/scripts/check-no-sim.sh" "$rel/ios/libpaw_core.a"
  fi
  # A separate target directory, so the default build above is not rebuilt
  # with other features next time.
  cargo build --lib --no-default-features --target-dir target/no-sim
  "$root/scripts/check-no-sim.sh" target/no-sim/debug/libpaw_core.a
  cargo run --bin uniffi-bindgen -- generate \
    --library target/no-sim/debug/libpaw_core.a --language swift --out-dir "$rel/bindings"
  mv "$rel/bindings/paw_core.swift" "$rel/"
  mv "$rel/bindings/paw_coreFFI.h" "$rel/headers/"
  mv "$rel/bindings/paw_coreFFI.modulemap" "$rel/headers/module.modulemap"
  rmdir "$rel/bindings"
fi

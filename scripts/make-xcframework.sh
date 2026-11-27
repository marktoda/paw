#!/usr/bin/env bash
# Packs the libraries from build-core.sh into app/build/PawCore.xcframework.
# macOS only.
#
# Each slice has its own library's header: the device slice the core
# without the `sim` feature, the simulator slice the core with it.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
core="$root/app/build/core"

rm -rf "$root/app/build/PawCore.xcframework"
xcodebuild -create-xcframework \
  -library "$core/ios/libpaw_core.a" -headers "$core/device-headers" \
  -library "$core/ios-sim/libpaw_core.a" -headers "$core/headers" \
  -output "$root/app/build/PawCore.xcframework"

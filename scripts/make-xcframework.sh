#!/usr/bin/env bash
# Packs the libraries from build-core.sh into app/build/PawCore.xcframework
# and generates the Xcode project. macOS only.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
core="$root/app/build/core"

rm -rf "$root/app/build/PawCore.xcframework"
xcodebuild -create-xcframework \
  -library "$core/ios/libpaw_core.a" -headers "$core/headers" \
  -library "$core/ios-sim/libpaw_core.a" -headers "$core/headers" \
  -output "$root/app/build/PawCore.xcframework"

cd "$root/app"
xcodegen generate

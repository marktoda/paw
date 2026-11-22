#!/usr/bin/env bash
# Runs the UI tests in a simulator and leaves a compact report in
# app/build/report/: errors.txt, summary.json, tests.json, screenshots/.
# Usage: ci-test.sh iPhone|iPad
set -uo pipefail
family="${1:-iPhone}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root/app"

report="build/report"
rm -rf "$report" build/result.xcresult
mkdir -p "$report/screenshots"

# Newest available simulator of the requested family.
udid=$(xcrun simctl list devices available -j | python3 -c '
import json, sys
family = sys.argv[1]
runtimes = json.load(sys.stdin)["devices"]
best = None
for runtime in sorted(runtimes):
    if "iOS" not in runtime:
        continue
    for device in runtimes[runtime]:
        if device["name"].startswith(family):
            best = device
print(best["udid"] if best else "")
' "$family")
if [ -z "$udid" ]; then
  echo "No $family simulator available" | tee "$report/errors.txt"
  exit 1
fi
xcrun simctl list devices available | grep "$udid" | tee "$report/device.txt"

xcodebuild test \
  -project Paw.xcodeproj -scheme Paw \
  -destination "platform=iOS Simulator,id=$udid" \
  -derivedDataPath build/derived \
  -resultBundlePath build/result.xcresult \
  CODE_SIGNING_ALLOWED=NO > "$report/xcodebuild.log" 2>&1
status=$?

# Compiler and linker errors, deduplicated, are the first thing to read.
grep -E "(error:|ld: |Undefined symbols|\*\* (BUILD|TEST) )" "$report/xcodebuild.log" \
  | sed "s#$root/##g" | awk '!seen[$0]++' > "$report/errors.txt"
grep -E "Test Case .* (passed|failed)" "$report/xcodebuild.log" > "$report/cases.txt" || true

if [ -d build/result.xcresult ]; then
  xcrun xcresulttool get test-results summary --path build/result.xcresult --compact \
    > "$report/summary.json" 2>/dev/null || true
  xcrun xcresulttool get test-results tests --path build/result.xcresult --compact \
    > "$report/tests.json" 2>/dev/null || true
  xcrun xcresulttool export attachments --path build/result.xcresult \
    --output-path "$report/screenshots" > /dev/null 2>&1 || true
fi

echo "--- errors"; cat "$report/errors.txt"
echo "--- cases"; cat "$report/cases.txt"
exit $status

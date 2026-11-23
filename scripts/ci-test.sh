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

udid="${PAW_SIM:-$("$root/scripts/ci-sim.sh" "$family")}"
if [ -z "$udid" ]; then
  echo "No $family simulator available" | tee "$report/errors.txt"
  exit 1
fi
xcrun simctl list devices available | grep "$udid" | tee "$report/device.txt"

# PAW_ONLY narrows the run to the tests being worked on, e.g.
# "GalleryTests KeyTests/testReadKeyShowsIdentityWithoutAPin".
only=()
for name in ${PAW_ONLY:-}; do
  only+=("-only-testing:PawUITests/$name")
done
# PAW_SKIP leaves classes out, for a run split across machines.
for name in ${PAW_SKIP:-}; do
  only+=("-skip-testing:PawUITests/$name")
done

# Where the minutes go, so the loop can be made faster with evidence.
timing="$report/timing.txt"
mark() { echo "$(date +%s) $1" >> "$timing"; }

mark build-start
xcodebuild build-for-testing \
  -project Paw.xcodeproj -scheme Paw \
  -destination "platform=iOS Simulator,id=$udid" \
  -derivedDataPath build/derived \
  CODE_SIGNING_ALLOWED=NO COMPILER_INDEX_STORE_ENABLE=NO > "$report/xcodebuild.log" 2>&1
status=$?
mark build-end

if [ $status -eq 0 ]; then
  # A cold simulator takes minutes to boot on a hosted runner. The workflow
  # starts it before the build; this waits for whatever is left.
  xcrun simctl bootstatus "$udid" -b > "$report/boot.log" 2>&1
  mark simulator-ready
  xcodebuild test-without-building \
    -project Paw.xcodeproj -scheme Paw \
    -destination "platform=iOS Simulator,id=$udid" \
    -derivedDataPath build/derived \
    -resultBundlePath build/result.xcresult \
    ${only[@]+"${only[@]}"} \
    CODE_SIGNING_ALLOWED=NO >> "$report/xcodebuild.log" 2>&1
  status=$?
  mark tests-end
fi


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

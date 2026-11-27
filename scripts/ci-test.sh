#!/usr/bin/env bash
# Runs UI tests (app/PawUITests) and unit tests (app/PawTests) in a simulator
# and leaves a compact report in app/build/report/, which the workflow
# uploads as `report-<family>-<part>` and scripts/ci-report.sh reads:
#   device.txt     the simulator it ran on
#   timing.txt     `<unix seconds> <mark>` per stage
#   errors.txt     compiler, linker and xcodebuild errors
#   cases.tsv      one line per test: module, class, test, passed|failed, seconds
#   summary.json   xcresulttool's summary, with each failure's message
#   screenshots/   the tests' attachments
#   crashes/       crash reports of the app or the extension from this run
#   xcodebuild.log, boot.log, tests.json   the raw output, for digging
# Usage: ci-test.sh iPhone|iPad
#
# What runs is always given:
#   PAW_ONLY  test names (scripts/test-id.sh), UI or unit, e.g.
#             "GalleryTests KeyTests/testReadKeyShowsIdentityWithoutAPin";
#             empty runs every UI test, less PAW_SKIP
#   PAW_SKIP  UI test classes or Class/test names to leave out
#   PAW_UNIT  1 adds every unit test (they take seconds), 0 adds none
set -uo pipefail
family="${1:-iPhone}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root/app"

report=build/report
result=build/result.xcresult
rm -rf "$report" "$result"
mkdir -p "$report/screenshots" "$report/crashes"

case "${PAW_UNIT:-}" in
  0|1) ;;
  *) echo "ci-test: PAW_UNIT must be 1 or 0" | tee "$report/errors.txt" >&2; exit 2 ;;
esac

udid="${PAW_SIM:-$("$root/scripts/ci-sim.sh" "$family")}"
if [ -z "$udid" ]; then
  echo "No $family simulator available" | tee "$report/errors.txt"
  exit 1
fi
xcrun simctl list devices available | grep "$udid" | tee "$report/device.txt"

ids="$("$root/scripts/test-id.sh" ${PAW_ONLY:-})"
if [ -n "$ids" ] && [ "$PAW_UNIT" = 1 ]; then
  ids="$ids"$'\n'PawTests
fi
selection=()
for id in $(sort -u <<<"$ids"); do
  selection+=("-only-testing:$id")
done
if [ -z "$ids" ] && [ "$PAW_UNIT" = 0 ]; then
  selection+=("-skip-testing:PawTests")
fi
for name in ${PAW_SKIP:-}; do
  selection+=("-skip-testing:PawUITests/$name")
done

xcode=(-project Paw.xcodeproj -scheme Paw
  -destination "platform=iOS Simulator,id=$udid"
  -derivedDataPath build/derived)

timing="$report/timing.txt"
mark() { echo "$(date +%s) $1" >> "$timing"; }

mark build-start
xcodebuild build-for-testing "${xcode[@]}" \
  CODE_SIGNING_ALLOWED=NO COMPILER_INDEX_STORE_ENABLE=NO > "$report/xcodebuild.log" 2>&1
status=$?
mark build-end

if [ $status -eq 0 ]; then
  # A cold simulator takes minutes to boot on a hosted runner. The workflow
  # starts it before the build; this waits for whatever is left.
  xcrun simctl bootstatus "$udid" -b > "$report/boot.log" 2>&1
  mark simulator-ready
  xcodebuild test-without-building "${xcode[@]}" -resultBundlePath "$result" \
    ${selection[@]+"${selection[@]}"} \
    CODE_SIGNING_ALLOWED=NO >> "$report/xcodebuild.log" 2>&1
  status=$?
  mark tests-end
fi

grep -E "(error:|ld: |Undefined symbols|\*\* (BUILD|TEST) )" "$report/xcodebuild.log" \
  | sed "s#$root/##g" | awk '!seen[$0]++' > "$report/errors.txt"
# XCTest's line for each finished test:
#   Test Case '-[PawUITests.KeyTests testReadKey]' passed (12.345 seconds).
tab=$'\t'
sed -nE "s/.*Test Case '-\[([A-Za-z0-9_]+)\.([A-Za-z0-9_]+) ([A-Za-z0-9_]+)\]' (passed|failed) \(([0-9.]+) seconds\).*/\1$tab\2$tab\3$tab\4$tab\5/p" \
  "$report/xcodebuild.log" > "$report/cases.tsv"

if [ -d "$result" ]; then
  xcrun xcresulttool get test-results summary --path "$result" --compact \
    > "$report/summary.json" 2>/dev/null || true
  xcrun xcresulttool get test-results tests --path "$result" --compact \
    > "$report/tests.json" 2>/dev/null || true
  xcrun xcresulttool export attachments --path "$result" \
    --output-path "$report/screenshots" > /dev/null 2>&1 || true
fi

# A test that ends with "is not running" says nothing about why; a crash
# report of the app or the extension from this run may.
find ~/Library/Logs/DiagnosticReports -maxdepth 1 \( -name "Paw*.ips" -o -name "Paw*.crash" \) -mmin -120 \
  -exec cp {} "$report/crashes/" \; 2>/dev/null || true
ls "$report/crashes" | sed "s/^/--- crash report: /"

echo "--- errors"; cat "$report/errors.txt"
echo "--- cases"; awk -F'\t' '{ printf "%s %s/%s (%ss)\n", $4, $2, $3, $5 }' "$report/cases.tsv"
exit $status

#!/usr/bin/env bash
# Downloads the report of a GitHub run (default: the latest) and prints what
# matters: which jobs failed, compiler errors, and each UI test's outcome.
# Usage: ci-report.sh [run-id] [--wait]
set -uo pipefail
cd "$(dirname "$0")/.."
run=""; wait=0
for arg in "$@"; do
  case "$arg" in --wait) wait=1 ;; *) run="$arg" ;; esac
done
[ -n "$run" ] || run=$(gh run list --limit 1 --json databaseId -q '.[0].databaseId')
[ "$wait" = 1 ] && gh run watch "$run" --interval 20 > /dev/null 2>&1

gh run view "$run" --json conclusion,headSha,jobs -q \
  '"run '"$run"' \(.headSha[0:7]): \(.conclusion)", (.jobs[] | "  \(.name): \(.conclusion)" + ((
    [.steps[] | select(.conclusion == "failure") | .name] | if length > 0 then " (failed at: " + join(", ") + ")" else "" end)))'

dest="${TMPDIR:-/tmp}/paw-ci/$run"
rm -rf "$dest"; mkdir -p "$dest"
gh run download "$run" --dir "$dest" > /dev/null 2>&1 || true

for report in "$dest"/report-*; do
  [ -d "$report" ] || continue
  echo "== $(basename "$report") ($(cat "$report/device.txt" 2>/dev/null | xargs))"
  if [ -s "$report/errors.txt" ]; then sed 's/^/  /' "$report/errors.txt" | head -40; fi
  if [ -s "$report/cases.txt" ]; then
    sed -E "s/.*Test Case '-\[[A-Za-z]+\.([A-Za-z]+) ([A-Za-z0-9_]+)\]' (passed|failed) \(([0-9.]+) seconds\).*/  \3  \2 (\4s)/" "$report/cases.txt"
  fi
  if [ -s "$report/summary.json" ]; then
    python3 -I -c '
import json, sys
s = json.load(open(sys.argv[1]))
for f in s.get("testFailures", []):
    print("  FAIL", f.get("testName"), "-", f.get("failureText"))
' "$report/summary.json" 2>/dev/null
  fi
  shots=$(find "$report/screenshots" -name '*.png' 2>/dev/null | wc -l)
  echo "  $shots screenshots in $report/screenshots"
done

# Failures outside the UI tests (Rust build, project generation) only show in the log.
gh run view "$run" --log-failed 2>/dev/null \
  | grep -E "error(\[|:)|##\[error\]" | sed -E 's/^([^\t]*)\t[^\t]*\t[0-9TZ:.-]+ /  [\1] /' | awk '!seen[$0]++' | head -25

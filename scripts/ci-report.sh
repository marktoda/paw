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

# The parts' reports (scripts/ci-test.sh lists their files), uploaded by
# .github/workflows/build.yml as `report-<family>-<part>`.
for report in "$dest"/report-*; do
  [ -d "$report" ] || continue
  echo "== $(basename "$report") ($(cat "$report/device.txt" 2>/dev/null | xargs))"
  if [ -s "$report/timing.txt" ]; then
    awk '$1 ~ /^[0-9]+$/ { if (p) printf "  %s took %ds\n", $2, $1 - p; p = $1 }' "$report/timing.txt"
  fi
  if [ -s "$report/errors.txt" ]; then sed 's/^/  /' "$report/errors.txt" | head -40; fi
  if [ -s "$report/cases.tsv" ]; then
    awk -F'\t' '{ printf "  %s  %s (%ss)\n", $4, $3, $5 }' "$report/cases.tsv"
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

# Across all parts, per test module: the total, each class's total and the
# slowest tests, to balance the split in .github/workflows/build.yml and to
# see what to speed up next.
cat "$dest"/report-*/cases.tsv 2>/dev/null | python3 -I -c '
import sys
modules = {}
for line in sys.stdin:
    module, cls, test, outcome, seconds = line.rstrip("\n").split("\t")
    modules.setdefault(module, []).append((float(seconds), cls, test, outcome))
for module, cases in sorted(modules.items()):
    failed = len([1 for *_, outcome in cases if outcome != "passed"])
    print("== %s: %d tests, %.0fs in all, %d failed" % (module, len(cases), sum(t for t, *_ in cases), failed))
    classes = {}
    for t, c, *_ in cases: classes.setdefault(c, []).append(t)
    for c, ts in sorted(classes.items(), key=lambda kv: -sum(kv[1])):
        print("  %-16s %3d tests %6.0fs  %4.0fs each" % (c, len(ts), sum(ts), sum(ts) / len(ts)))
    print("  slowest:")
    for t, c, n, _ in sorted(cases, reverse=True)[:10]:
        print("  %6.1fs %s/%s" % (t, c, n))
'

# Failures outside the UI tests (Rust build, project generation) only show in the log.
gh run view "$run" --log-failed 2>/dev/null \
  | grep -E "error(\[|:)|##\[error\]" | sed -E 's/^([^\t]*)\t[^\t]*\t[0-9TZ:.-]+ /  [\1] /' | awk '!seen[$0]++' | head -25

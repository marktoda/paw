#!/usr/bin/env bash
# The app's debug launch arguments, from their two declared lists:
# `KeyLaunchArgument` in app/Shared/LaunchArguments.swift (read by both
# processes) and `LaunchArgument` in app/Paw/AppLaunchArguments.swift (the
# app's own). Prints each name (`pawFixture`), one per line, after checking
# that
# - every `-paw…` argument a UI test passes (app/PawUITests) is declared, and
# - every declared argument is read somewhere in the app, through one of the
#   accessors `DebugLaunchArgument` declares (`fixture.bool`).
# It reads the Swift by its spelling (`case fixture = "pawFixture"`,
# `var bool: Bool {`), so it fails when a list has no declaration it can
# read or a line it cannot, rather than check less.
# scripts/check-no-sim.sh --app looks for every name in the device build.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
shared="$root/app/Shared/LaunchArguments.swift"
lists=("$shared" "$root/app/Paw/AppLaunchArguments.swift")

declaration='^[[:space:]]*case ([A-Za-z]+) = "(paw[A-Za-z]+)"$'
declared=""
for list in "${lists[@]}"; do
  found="$(sed -nE "s/$declaration/\1 \2/p" "$list")"
  [[ -n "$found" ]] || { echo "launch-arguments: no declaration read in $list" >&2; exit 2; }
  unread="$(grep -E '"paw[A-Za-z]+"' "$list" | grep -vE "$declaration" || true)"
  [[ -z "$unread" ]] || { printf 'launch-arguments: %s has lines it cannot read:\n%s\n' "$list" "$unread" >&2; exit 2; }
  declared+="$found"$'\n'
done
declared="${declared%$'\n'}"

accessors="$(sed -nE 's/^    var ([a-z]+): .*\{$/\1/p' "$shared" | paste -sd '|' -)"
[[ -n "$accessors" ]] || { echo "launch-arguments: no accessor of DebugLaunchArgument read in $shared" >&2; exit 2; }

failed=0
while read -r name; do
  if ! grep -q -x -F "$name" <<<"$(cut -d' ' -f2 <<<"$declared")"; then
    echo "launch-arguments: the UI tests pass -$name, which no launch argument list declares" >&2
    failed=1
  fi
done < <(grep -rhoE '"-paw[A-Za-z]+"' "$root/app/PawUITests" | sed -E 's/^"-(.*)"$/\1/' | sort -u)

while read -r case name; do
  if ! grep -rqE "(^|[^A-Za-z0-9_])$case\.($accessors)([^A-Za-z0-9_]|$)" \
      "$root/app/Paw" "$root/app/Shared" "$root/app/PawAutoFill"; then
    echo "launch-arguments: $case ($name) is declared but nothing reads it" >&2
    failed=1
  fi
done <<<"$declared"

[[ "$failed" -eq 0 ]] || exit 1
cut -d' ' -f2 <<<"$declared"

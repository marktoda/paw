#!/usr/bin/env bash
# The app's debug launch arguments, from their one declared list,
# `LaunchArgument` in app/Shared/LaunchArguments.swift. Prints each name
# (`pawFixture`), one per line, after checking that
# - every `-paw…` argument a UI test passes (app/PawUITests) is declared, and
# - every declared argument is read somewhere in the app.
# scripts/check-no-sim.sh --app looks for every name in the device build.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
list="$root/app/Shared/LaunchArguments.swift"

declared="$(sed -nE 's/^[[:space:]]*case ([A-Za-z]+) = "(paw[A-Za-z]+)"$/\1 \2/p' "$list")"
[[ -n "$declared" ]] || { echo "launch-arguments: nothing declared in $list" >&2; exit 2; }

failed=0
while read -r name; do
  if ! grep -q -x -F "$name" <<<"$(cut -d' ' -f2 <<<"$declared")"; then
    echo "launch-arguments: the UI tests pass -$name, which LaunchArgument does not declare" >&2
    failed=1
  fi
done < <(grep -rhoE '"-paw[A-Za-z]+"' "$root/app/PawUITests" | sed -E 's/^"-(.*)"$/\1/' | sort -u)

while read -r case name; do
  if ! grep -rqE "(^|[^A-Za-z0-9_])$case\.(string|bool|int)([^A-Za-z0-9_]|$)" \
      "$root/app/Paw" "$root/app/Shared" "$root/app/PawAutoFill"; then
    echo "launch-arguments: LaunchArgument.$case ($name) is declared but nothing reads it" >&2
    failed=1
  fi
done <<<"$declared"

[[ "$failed" -eq 0 ]] || exit 1
cut -d' ' -f2 <<<"$declared"

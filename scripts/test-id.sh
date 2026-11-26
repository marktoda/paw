#!/usr/bin/env bash
# Prints, one per line, the xcodebuild test identifier of each test name
# given (`Class` or `Class/test`): a unit test class lives in
# app/PawTests/<Class>.swift, any other class is a UI test (app/PawUITests).
# "PawTests" names every unit test.
# Usage: test-id.sh NAME...
set -euo pipefail
tests="$(cd "$(dirname "$0")/../app/PawTests" && pwd)"
for name in "$@"; do
  if [ "$name" = PawTests ]; then
    echo PawTests
  elif [ -f "$tests/${name%%/*}.swift" ]; then
    echo "PawTests/$name"
  else
    echo "PawUITests/$name"
  fi
done

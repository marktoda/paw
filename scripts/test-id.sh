#!/usr/bin/env bash
# Prints, one per line, the xcodebuild test identifier of each test name
# given (`Class` or `Class/test`): a class that some file in app/PawTests
# declares as `class <Class>: XCTestCase` is a unit test, any other is a UI
# test (app/PawUITests). "PawTests" names every unit test.
# Usage: test-id.sh NAME...
set -euo pipefail
tests="$(cd "$(dirname "$0")/../app/PawTests" && pwd)"
for name in "$@"; do
  class="${name%%/*}"
  if [ "$name" = PawTests ]; then
    echo PawTests
  elif grep -qE "^(final )?class $class: XCTestCase" "$tests"/*.swift; then
    echo "PawTests/$name"
  else
    echo "PawUITests/$name"
  fi
done

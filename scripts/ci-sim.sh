#!/usr/bin/env bash
# Prints the UDID of the newest available simulator of a family.
# Usage: ci-sim.sh iPhone|iPad
set -euo pipefail
xcrun simctl list devices available -j | python3 -c '
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
' "${1:-iPhone}"

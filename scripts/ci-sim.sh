#!/usr/bin/env bash
# Prints the UDID of the newest available simulator of a family.
# Usage: ci-sim.sh iPhone|iPad
set -euo pipefail
xcrun simctl list devices available -j | python3 -I -c '
import json, re, sys
family = sys.argv[1]
runtimes = json.load(sys.stdin)["devices"]
# Runtimes are named like com.apple.CoreSimulator.SimRuntime.iOS-26-0.
def version(runtime):
    return [int(n) for n in re.findall(r"[0-9]+", runtime.rsplit("iOS", 1)[1])]
best = None
for runtime in sorted((r for r in runtimes if "iOS" in r), key=version):
    for device in runtimes[runtime]:
        if device["name"].startswith(family):
            best = device
print(best["udid"] if best else "")
' "${1:-iPhone}"

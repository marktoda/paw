#!/usr/bin/env bash
# Fails if a device build still carries the simulated hardware or the
# app's debug harnesses (design.md 5.5, rule 10 of 5.6).
#
#   scripts/check-no-sim.sh [--app] FILE...            the device build: nothing may be found
#   scripts/check-no-sim.sh --expect [--app] FILE...   a simulator build: everything must be found
#
# FILE is a library (`libpaw_core.a`) or a linked binary (the app's `Paw`,
# the extension's `PawAutoFill`, which have the core linked in).
#
# What it looks for:
# - symbols of the simulated key, keystore, clock, fixture and git host,
#   as Rust and as the UniFFI scaffolding Swift would call (`nm`);
# - the fixture store's passwords and the simulated host's private host key,
#   anywhere in the bytes (`grep -a`), which would mean the fixture was
#   compiled in even if no symbol names it;
# - with --app, the Swift debug surfaces: the launch arguments that switch
#   harnesses on and the names of the types the simulator, debug and
#   gallery folders declare at top level (private ones too), anywhere in
#   the bytes (type names are in Swift's metadata even when symbols are
#   gone).
#
# --expect keeps the patterns honest: run on a build that has the simulation
# (the simulator library; with --app, the Debug simulator app and
# extension), every pattern must match somewhere in FILE..., or the check
# fails naming the ones that matched nothing. With --app it checks only the
# app's patterns: the linker drops core code the app never calls, so the
# core's are checked on the library.
# For example, in core/:
#   cargo build --lib && ../scripts/check-no-sim.sh --expect target/debug/libpaw_core.a
#   cargo build --lib --no-default-features --target-dir target/no-sim \
#     && ../scripts/check-no-sim.sh target/no-sim/debug/libpaw_core.a
set -euo pipefail

expect=no
app=no
files=()
for arg in "$@"; do
  case "$arg" in
    --expect) expect=yes ;;
    --app) app=yes ;;
    -*) echo "check-no-sim: unknown option $arg" >&2; exit 2 ;;
    *) files+=("$arg") ;;
  esac
done
[[ ${#files[@]} -gt 0 ]] || { echo "usage: check-no-sim.sh [--expect] [--app] FILE..." >&2; exit 2; }
for file in "${files[@]}"; do
  [[ -f "$file" ]] || { echo "check-no-sim: no such file: $file" >&2; exit 2; }
done
command -v nm >/dev/null || { echo "check-no-sim: nm is needed" >&2; exit 2; }

# Mangled Rust names contain the module path pieces (`3sim`, `9SimRemote`),
# UniFFI's exported C functions contain the lower-cased type names. One
# pattern per thing, so --expect can say which one matched nothing.
symbols=(
  'simulatedkey|SimulatedKey'
  'simkeystore|SimKeystore'
  'simclock|SimClock'
  'simremote|SimRemote'
  'simgithub|SimGitHub'
  'write_fixture_store'
  'paw_core.*3sim[0-9]|paw_core::sim::'
)

# A few of the fixture store's passwords (core/src/sim/fixture.rs) and a
# line of the simulated host's ECDSA key (core/src/sim/remote.rs).
needles=(
  'tidal-Quartz-88-ferns'
  'Harbor_73_willow'
  'plum-cedar-otter-41'
  'Kestrel/ember/5150'
  'first-password-ever'
  'kB+w7P9NzlaYwa0zuD/P3nJG8zxGMSKapeTExcKjUTPp2L4xoi/2K9vmAAAAqE1dnpZNXZ'
)

# The app's debug surfaces, only with --app: every declared launch argument
# (scripts/launch-arguments.sh), any `-pawSim…` one, and every type the
# simulator, debug and gallery folders declare at the start of a line.
# Everything in those folders is under `#if DEBUG` (app/project.yml).
# Not scanned: types nested in those (short names such as `Row` that the
# device build has too), and what those folders' extensions add to types
# declared elsewhere (`View`, `EnvironmentValues`, `KeyReader`): a member's
# name is not in a stripped build.
app_needles=()
if [[ "$app" == yes ]]; then
  here="$(cd "$(dirname "$0")" && pwd)"
  arguments="$("$here/launch-arguments.sh")"
  folders=("$here"/../app/Paw/Simulator/*.swift "$here"/../app/Shared/Simulator/*.swift
    "$here"/../app/Paw/Debug/*.swift "$here"/../app/Paw/Gallery/*.swift)
  types="$(sed -nE 's/^((private|fileprivate) )?(final )?(class|struct|enum|actor) ([A-Za-z]+).*/\5/p' "${folders[@]}" | sort -u)"
  [[ -n "$arguments" && -n "$types" ]] || { echo "check-no-sim: no debug surfaces found in app/" >&2; exit 2; }
  app_needles=('pawSim' $arguments $types)
fi

if [[ "$app" == yes && "$expect" == yes ]]; then
  symbols=()
  needles=("${app_needles[@]}")
elif [[ "$app" == yes ]]; then
  needles+=("${app_needles[@]}")
fi

# `nm` may complain about members it cannot read, so its status is ignored,
# but every file has some symbols: none at all means nm read nothing.
names="$(mktemp)"
trap 'rm -f "$names"' EXIT
for file in "${files[@]}"; do
  listed="$(nm -a "$file" 2>/dev/null || true)"
  [[ -n "$listed" ]] || { echo "check-no-sim: nm read no symbols from $file" >&2; exit 2; }
  echo "$listed" >> "$names"
done

found=0
missing=()
for pattern in ${symbols[@]+"${symbols[@]}"}; do
  hits=$(grep -E -- "$pattern" "$names" || true)
  if [[ -n "$hits" ]]; then
    found=1
    if [[ "$expect" == no ]]; then
      echo "check-no-sim: symbols matching '$pattern', e.g.:" >&2
      sed -n 1,3p <<<"$hits" >&2
    fi
  else
    missing+=("symbol /$pattern/")
  fi
done

for needle in "${needles[@]}"; do
  hit=no
  for file in "${files[@]}"; do
    if LC_ALL=C grep -a -q -F -- "$needle" "$file"; then
      hit=yes
      if [[ "$expect" == no ]]; then echo "check-no-sim: $file contains '$needle'" >&2; fi
    fi
  done
  if [[ "$hit" == yes ]]; then
    found=1
  else
    missing+=("'$needle'")
  fi
done

if [[ "$expect" == yes ]]; then
  if [[ ${#missing[@]} -gt 0 ]]; then
    echo "check-no-sim: these patterns match nothing in a build that has the simulation, so they prove nothing:" >&2
    printf '  %s\n' "${missing[@]}" >&2
    exit 1
  fi
  echo "check-no-sim: every pattern matches the simulation build (${files[*]})"
  exit 0
fi

if [[ "$found" -ne 0 ]]; then
  exit 1
fi
echo "check-no-sim: no simulated hardware or debug harness in ${files[*]}"

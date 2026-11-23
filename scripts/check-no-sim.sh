#!/usr/bin/env bash
# Fails if a library built without the `sim` feature still carries the
# simulated hardware (design.md 5.5, rule 10 of 5.6).
#
#   scripts/check-no-sim.sh path/to/libpaw_core.a
#
# It looks for two things:
# - symbols of the simulated key, keystore, clock, fixture and git host,
#   as Rust and as the UniFFI scaffolding Swift would call (`nm`);
# - the fixture store's passwords and the simulated host's private host key,
#   anywhere in the bytes (`grep -a`), which would mean the fixture was
#   compiled in even if no symbol names it.
#
# A library built with the feature (the default) must fail this check; that
# is checked too, by the tests' own build, so the patterns stay honest:
#   cargo build --lib && scripts/check-no-sim.sh core/target/debug/libpaw_core.a   # fails
#   cargo build --lib --no-default-features && scripts/check-no-sim.sh ...         # passes
set -euo pipefail

lib="${1:?usage: check-no-sim.sh <library>}"
[[ -f "$lib" ]] || { echo "check-no-sim: no such file: $lib" >&2; exit 2; }
command -v nm >/dev/null || { echo "check-no-sim: nm is needed" >&2; exit 2; }

found=0

# Mangled Rust names contain the module path pieces (`3sim`, `9SimRemote`),
# UniFFI's exported C functions contain the lower-cased type names.
symbols='simulatedkey|SimulatedKey|simkeystore|SimKeystore|simclock|SimClock|simremote|SimRemote|write_fixture_store|paw_core.*3sim[0-9]|paw_core::sim::'
# `nm` may complain about members it cannot read; only its output counts.
hits=$( (nm -a "$lib" 2>/dev/null || true) | grep -E "$symbols" || true)
if [[ -n "$hits" ]]; then
  echo "check-no-sim: simulation symbols in $lib, e.g.:" >&2
  sed -n 1,5p <<<"$hits" >&2
  found=1
fi

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
for needle in "${needles[@]}"; do
  if LC_ALL=C grep -a -q -F -- "$needle" "$lib"; then
    echo "check-no-sim: $lib contains simulation data: $needle" >&2
    found=1
  fi
done

if [[ "$found" -ne 0 ]]; then
  exit 1
fi
echo "check-no-sim: $lib has no simulated hardware"

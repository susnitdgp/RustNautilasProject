#!/usr/bin/env bash
# Manual foreground launcher for MCX Crude PURE Squeeze Momentum v2.28.3.
set -euo pipefail
umask 077
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd -- "$ROOT"
if (( $# != 0 )); then
    printf 'Usage: bash deploy/run-squeeze-momentum-live.sh\n' >&2
    exit 2
fi
if [[ ! -x target/release/kite-node ]]; then
    printf 'Missing executable: target/release/kite-node\n' >&2
    exit 1
fi
printf 'Starting LIVE MCX Crude PURE Squeeze Momentum v2.28.3.\n'
printf 'Strategy: config/production-squeeze-momentum.json; broker: config/kite-production.json.\n'
printf 'Trade decisions are confirmed-bar only; dashboard diagnostics may update tick-by-tick.\n'
printf 'The runtime remains alive through the 23:15 strategy square-off and shuts down before MCX close.\n'
printf 'Keep this terminal open. Ctrl-C requests graceful shutdown; verify the final position in Kite.\n'
exec ./target/release/kite-node native-squeeze-momentum-kite-production \
    config/production-squeeze-momentum.json config/kite-production.json

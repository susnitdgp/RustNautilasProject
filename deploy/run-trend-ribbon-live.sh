#!/usr/bin/env bash
# Manual foreground launcher for real BOSWaves Trend Ribbon execution.
set -euo pipefail
umask 077
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd -- "$project_dir"
if (( $# != 0 )); then
    printf 'Usage: bash deploy/run-trend-ribbon-live.sh\n' >&2
    exit 2
fi
if [[ ! -x target/release/kite-node ]]; then
    printf 'Missing executable: target/release/kite-node\n' >&2
    exit 1
fi
printf 'Starting LIVE BOSWaves Trend Ribbon using the JSON candle interval: MIS, protected MARKET orders.\n'
printf 'Strategy: config/production-trend-ribbon.json; broker: config/kite-production.json.\n'
printf 'Production cutoff is enforced at 23:00 IST by the 30-minute market-close buffer.\n'
printf 'Keep this terminal open. Ctrl-C requests graceful shutdown; verify the final position in Kite.\n'
exec ./target/release/kite-node native-trend-ribbon-kite-production \
    config/production-trend-ribbon.json config/kite-production.json

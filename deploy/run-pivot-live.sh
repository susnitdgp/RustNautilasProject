#!/usr/bin/env bash
# Manual foreground launcher for real Pivot Point SuperTrend execution.
set -euo pipefail
umask 077
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd -- "$project_dir"
if (( $# != 0 )); then
    printf 'Usage: bash deploy/run-pivot-live.sh\n' >&2
    exit 2
fi
if [[ ! -x target/release/kite-node ]]; then
    printf 'Missing executable: target/release/kite-node\n' >&2
    exit 1
fi
printf 'Starting LIVE Pivot Point SuperTrend: 5m, MIS, protected MARKET orders.\n'
printf 'Strategy: config/backup/production-pivot-supertrend.json; broker: config/kite-production.json.\n'
printf 'The application enforces strategy/account gates and the 30-minute market-close buffer.\n'
printf 'Keep this terminal open. Ctrl-C requests graceful shutdown; verify the final position in Kite.\n'
exec ./target/release/kite-node native-pivot-kite-production \
    config/backup/production-pivot-supertrend.json config/kite-production.json

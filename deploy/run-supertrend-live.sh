#!/usr/bin/env bash
# Manual foreground launcher: starts real trading when the broker gates permit it.
set -euo pipefail
umask 077
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd -- "$project_dir"
if (( $# != 0 )); then
    printf 'Usage: bash deploy/run-supertrend-live.sh\n' >&2
    exit 2
fi
if [[ ! -x target/release/kite-node ]]; then
    printf 'Missing executable: target/release/kite-node\n' >&2
    exit 1
fi
printf 'Starting LIVE Supertrend: 5m, MIS, protected MARKET orders.\n'
printf 'Configuration/account gates are checked by the application.\n'
printf 'Keep this terminal open. Ctrl-C requests graceful shutdown; verify the final position.\n'
# exec preserves the program exit code and delivers terminal signals directly.
# No config edits, automatic restarts, owner-key cleanup or implicit rebuilds.
exec ./target/release/kite-node native-supertrend-kite-production \
    config/backup/production-supertrend.json config/kite-production.json

#!/usr/bin/env bash
set -euo pipefail
umask 077
cd /home/ubuntu/RustNautilasProject
if (( $# != 0 )); then
  printf 'Usage: bash deploy/run-smbc-live.sh\n' >&2
  exit 2
fi
if [[ ! -x target/release/kite-node ]]; then
  printf 'Missing executable: target/release/kite-node\n' >&2
  exit 1
fi
printf 'Starting LIVE Smart Money Breakout Channels v1.7.\n'
printf 'Strategy: config/production-smbc.json; broker: config/kite-production.json.\n'
printf 'Live orders require BOTH JSON gates to be enabled.\n'
exec ./target/release/kite-node native-smbc-kite-production config/production-smbc.json config/kite-production.json

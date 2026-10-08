#!/usr/bin/env bash
set -euo pipefail

cd /home/ubuntu/RustNautilasProject

BIN="./target/release/kite-node"
STRATEGY="config/production-ilrc-live.json"
BROKER="config/kite-ilrc-live.json"

[[ -x "$BIN" ]] || { echo 'Release binary missing.' >&2; exit 1; }
[[ -f "$STRATEGY" && -f "$BROKER" ]] || { echo 'Live configuration files missing.' >&2; exit 1; }

"$BIN" native-ilrc-live-preflight "$STRATEGY" "$BROKER"

echo 'WARNING: This will place REAL Zerodha Kite orders.'
read -r -p 'Type LIVE to proceed: ' CONFIRM
if [[ "$CONFIRM" != 'LIVE' ]]; then
    echo 'Cancelled.'
    exit 1
fi

exec "$BIN" native-ilrc-nautilus-live "$STRATEGY" "$BROKER"

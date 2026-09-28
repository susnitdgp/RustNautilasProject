#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
CONFIG="config/production-squeeze-momentum.json"
DURATION="${1:-}"

if [[ -z "$DURATION" || ! "$DURATION" =~ ^[0-9]+$ || "$DURATION" -lt 60 || "$DURATION" -gt 86400 ]]; then
  echo "Usage: $0 SECONDS   (60..86400)" >&2
  exit 2
fi

python3 - "$CONFIG" <<'PY'
import json,sys
v=json.load(open(sys.argv[1]))
assert v["strategy"]=="squeeze_momentum_lazybear_v2283"
assert v["interval"]=="5minute"
assert v["live_orders_enabled"] is False
print("recorder safety: PASS (market data only; execution client absent)")
PY

exec ./target/debug/kite-node native-squeeze-momentum-record "$CONFIG" "$DURATION"

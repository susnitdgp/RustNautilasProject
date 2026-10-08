#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT=/home/ubuntu/RustNautilasProject
cd "$ROOT"

BIN=./target/release/kite-node
STRATEGY=config/production-ilrc.json
BROKER=config/kite-production.json

if [[ ! -x "$BIN" ]]; then
  printf 'Missing release executable: %s\n' "$BIN" >&2
  exit 1
fi

printf '%s\n' '--- ILRC combined production configuration ---'
"$BIN" native-ilrc-production-check "$STRATEGY" "$BROKER"

printf '%s\n' '--- Golden-date regression: 2026-10-07 ---'
OUT="$("$BIN" native-ilrc-config-date "$STRATEGY" 2026-10-07)"
printf '%s\n' "$OUT"

python3 - "$OUT" <<'PY'
import json, sys
x = json.loads(sys.argv[1])
assert x['strategy'] == 'ILRC_COMBINED'
assert x['closed_trades'] == 5
assert x['wins'] == 3
assert x['losses'] == 0
assert x['breakeven'] == 2
assert abs(x['gross_points'] - 68.9272488092829) < 1e-9
assert x['live_orders_enabled'] is False
reasons = [t['reason'] for t in x['trades']]
assert 'A_TP' in reasons
assert reasons.count('B_TP') == 2
assert reasons.count('B_BE') == 2
print('golden regression: PASS')
PY

printf '%s\n' '--- Five-second production-shadow smoke test ---'
"$BIN" native-ilrc-shadow "$STRATEGY" 5

printf '%s\n' 'ILRC combined production-shadow verification: PASS'

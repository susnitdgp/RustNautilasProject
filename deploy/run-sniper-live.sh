#!/usr/bin/env bash
# run-sniper-live.sh v1.0.0
# Precision Sniper CRUDEOILM LIVE run: places REAL Zerodha orders (lots, TP1/TP2 lots,
# timeframe, blackouts all from config/sniper-crudeoilm.json; MIS, square-off from the same file).
# Gates: --features live-orders build, slot enabled + live_orders_enabled in
# config/portfolio-production.json, live_orders_enabled and max_lots >= lots in
# config/kite-production.json, and the operator typing LIVE below.
set -euo pipefail
cd /home/ubuntu/RustNautilasProject
BIN="./target/release/kite-node"
PORTFOLIO="config/portfolio-production.json"
SLOT="crudeoilm-sniper-202610"
BROKER="config/kite-production.json"
[[ -x "$BIN" ]] || { echo 'Release binary missing: cargo build --release --features live-orders' >&2; exit 1; }
[[ -f "$PORTFOLIO" && -f "$BROKER" ]] || { echo 'Live configuration files missing.' >&2; exit 1; }
"$BIN" native-sniper-live-preflight "$PORTFOLIO" "$SLOT" "$BROKER"
LOTS=$(python3 -c 'import json;print(json.load(open("config/sniper-crudeoilm.json"))["lots"])')
echo "WARNING: This will place REAL Zerodha Kite orders for CRUDEOILM (${LOTS} lots, MIS)."
read -r -p 'Type LIVE to proceed: ' CONFIRM
[[ "$CONFIRM" == 'LIVE' ]] || { echo 'Cancelled.'; exit 1; }
mkdir -p logs
LOG="logs/sniper-live-$(TZ=Asia/Kolkata date +%F).jsonl"
echo "Dashboard starting; JSON events -> $LOG"
# stdout (JSON) goes straight to the file: no pipe, so Ctrl+C reaches only the
# runner, which flattens and stops cleanly. The dashboard draws on stderr.
exec "$BIN" native-sniper-live "$PORTFOLIO" "$SLOT" "$BROKER" >> "$LOG"

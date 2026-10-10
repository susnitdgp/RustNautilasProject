#!/usr/bin/env bash
# run-sats-live.sh v1.1.0
# SATS CRUDEOILM LIVE run: places REAL Zerodha orders (1 lot, MIS, square-off 23:15 IST).
# Gates: --features live-orders build, slot enabled + live_orders_enabled in
# config/portfolio-production.json, live_orders_enabled in config/kite-production.json,
# and the operator typing LIVE below.
set -euo pipefail
cd /home/ubuntu/RustNautilasProject
BIN="./target/release/kite-node"
PORTFOLIO="config/portfolio-production.json"
SLOT="crudeoilm-sats-202610"
BROKER="config/kite-production.json"
[[ -x "$BIN" ]] || { echo 'Release binary missing: cargo build --release --features live-orders' >&2; exit 1; }
[[ -f "$PORTFOLIO" && -f "$BROKER" ]] || { echo 'Live configuration files missing.' >&2; exit 1; }
"$BIN" native-sats-live-preflight "$PORTFOLIO" "$SLOT" "$BROKER"
echo 'WARNING: This will place REAL Zerodha Kite orders for CRUDEOILM (1 lot, MIS).'
read -r -p 'Type LIVE to proceed: ' CONFIRM
[[ "$CONFIRM" == 'LIVE' ]] || { echo 'Cancelled.'; exit 1; }
mkdir -p logs
LOG="logs/sats-live-$(TZ=Asia/Kolkata date +%F).jsonl"
echo "Live state -> dashboard Redis (config/dashboard.json); JSON events -> $LOG"
# stdout (JSON) goes straight to the file: no pipe, so Ctrl+C reaches only the
# runner, which flattens and stops cleanly. Short notes go to stderr; the live
# state is published to the dashboard Redis by a background thread.
exec "$BIN" native-sats-live "$PORTFOLIO" "$SLOT" "$BROKER" >> "$LOG"

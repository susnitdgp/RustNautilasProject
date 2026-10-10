#!/usr/bin/env bash
# run-sniper-paper.sh v1.1.0
# Precision Sniper CRUDEOILM PAPER run: live Kite market data, orders filled by the
# native Kite mock (never touches Zerodha's order API). Same JSON config as live.
set -euo pipefail
cd /home/ubuntu/RustNautilasProject
BIN="./target/release/kite-node"
PORTFOLIO="config/portfolio-production.json"
SLOT="crudeoilm-sniper-202610"
[[ -x "$BIN" ]] || { echo 'Release binary missing: cargo build --release --features live-orders' >&2; exit 1; }
mkdir -p logs
LOG="logs/sniper-paper-$(TZ=Asia/Kolkata date +%F).jsonl"
echo "Live state -> dashboard Redis (config/dashboard.json); JSON events -> $LOG"
exec "$BIN" native-sniper-paper "$PORTFOLIO" "$SLOT" >> "$LOG"

#!/usr/bin/env bash
# run-sats-paper.sh v1.1.0
# SATS CRUDEOILM paper run: live Kite data, Kite mock execution. Never sends broker orders.
set -euo pipefail
cd /home/ubuntu/RustNautilasProject
BIN="./target/release/kite-node"
[[ -x "$BIN" ]] || { echo 'Release binary missing: cargo build --release' >&2; exit 1; }
mkdir -p logs
LOG="logs/sats-paper-$(TZ=Asia/Kolkata date +%F).jsonl"
echo "Live state -> dashboard Redis (config/dashboard.json); JSON events -> $LOG"
exec "$BIN" native-sats-paper config/portfolio-development.example.json crudeoilm-sats-202610 >> "$LOG"

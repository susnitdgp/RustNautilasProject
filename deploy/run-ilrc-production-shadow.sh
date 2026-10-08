#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT=/home/ubuntu/RustNautilasProject
cd "$ROOT"

if (( $# > 1 )); then
  printf 'Usage: bash deploy/run-ilrc-production-shadow.sh [seconds]\n' >&2
  exit 2
fi

SECONDS_TO_RUN="${1:-86360}"
if ! [[ "$SECONDS_TO_RUN" =~ ^[0-9]+$ ]] || (( SECONDS_TO_RUN < 5 || SECONDS_TO_RUN > 86360 )); then
  printf 'Duration must be an integer from 5 to 86360 seconds.\n' >&2
  exit 2
fi

BIN=./target/release/kite-node
STRATEGY=config/production-ilrc.json
BROKER=config/kite-production.json

if [[ ! -x "$BIN" ]]; then
  printf 'Missing release executable: %s\nBuild with: cargo build --release --locked -p kite-node\n' "$BIN" >&2
  exit 1
fi
if [[ ! -r "$STRATEGY" || ! -r "$BROKER" ]]; then
  printf 'Missing production configuration.\n' >&2
  exit 1
fi

"$BIN" native-ilrc-production-check "$STRATEGY" "$BROKER"

printf 'Starting ILRC COMBINED production shadow.\n'
printf 'Setup A: reversal + 1R BE; Setup B: continuation + 3R TP + 1R BE.\n'
printf 'Order execution is disabled; no execution client is loaded.\n'

exec "$BIN" native-ilrc-shadow "$STRATEGY" "$SECONDS_TO_RUN"

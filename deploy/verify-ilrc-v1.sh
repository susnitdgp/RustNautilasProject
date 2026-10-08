#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo run --locked -p kite-node -- native-ilrc-production-check config/production-ilrc.json config/kite-production.json
cargo run --locked -p kite-node -- native-ilrc-config-date config/production-ilrc.json 2026-10-07

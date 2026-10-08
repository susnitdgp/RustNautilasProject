#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
SECONDS_TO_RUN="${1:-3600}"
exec target/debug/kite-node native-ilrc-shadow config/production-ilrc.json "$SECONDS_TO_RUN"

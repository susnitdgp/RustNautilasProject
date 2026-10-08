#!/usr/bin/env bash
set -euo pipefail
cd /home/ubuntu/RustNautilasProject
SECONDS="${1:-3600}"
exec ./target/debug/kite-node native-smbc-record config/production-smbc.json "$SECONDS"

#!/usr/bin/env bash
set -euo pipefail
cd /home/ubuntu/RustNautilasProject
./target/release/kite-node native-ilrc-live-readiness config/production-ilrc.json config/kite-production.json config/ilrc-live-integration.json
printf '%s\n' 'Dry-run integration safety validated. LIVE EXECUTION NOT IMPLEMENTED AND NOT ENABLED.'

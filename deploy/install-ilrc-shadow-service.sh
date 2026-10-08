#!/usr/bin/env bash
set -euo pipefail

ROOT=/home/ubuntu/RustNautilasProject
UNIT=ilrc-combined-shadow.service

cd "$ROOT"
sudo install -m 0644 "deploy/$UNIT" "/etc/systemd/system/$UNIT"
sudo systemctl daemon-reload

printf 'Installed %s but did NOT enable or start it.\n' "$UNIT"
printf 'Verify first: bash deploy/verify-ilrc-production.sh\n'
printf 'Then, when ready for shadow deployment: sudo systemctl enable --now %s\n' "$UNIT"

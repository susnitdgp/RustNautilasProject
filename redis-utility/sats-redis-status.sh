#!/usr/bin/env bash
# sats-redis-status.sh v2.0.0
# Read-only view of the SATS slot (kite-node 2.21.0+: no journal, no lease): whether a
# run holds the slot's lock file (live and paper) and the shared order budget in Redis.
# Never prints or touches the Kite credential keys.
# Usage: ./redis-utility/sats-redis-status.sh [slot] [kite_user]
set -uo pipefail
SLOT="${1:-crudeoilm-sats-202610}"
USER_ID="${2:-NVC171}"
LOCK_DIR="${KITE_LOCK_DIR:-$HOME/.local/state/kite-node/locks}"

lock() {
  local file="$LOCK_DIR/$1.lock"
  if [[ ! -f "$file" ]]; then
    echo "  $file  (never used)"
  elif flock -n "$file" true 2>/dev/null; then
    echo "  $file  free"
  else
    echo "  $file  HELD ($(cat "$file" 2>/dev/null))"
  fi
}

echo "SATS slot: $SLOT"
echo
echo "Instance locks (HELD = a run is active for this slot)"
lock "kite-prod-$SLOT-$USER_ID"
lock "kite-dev-$SLOT-PAPER"

echo
echo "Shared order budget"
key="kite-prod:v1:{account-$USER_ID}:order-budget"
if [[ "$(redis-cli EXISTS "$key")" == "1" ]]; then
  echo "  $key  type=$(redis-cli TYPE "$key")"
else
  echo "  $key  (not created yet)"
fi

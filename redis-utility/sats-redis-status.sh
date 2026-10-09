#!/usr/bin/env bash
# sats-redis-status.sh v1.0.1
# Read-only view of the SATS slot's Redis state: account lease (live and
# paper), today's run ledgers and the shared order budget. Never prints or
# touches the Kite credential keys.
# Usage: ./redis-utility/sats-redis-status.sh [slot] [kite_user]
set -uo pipefail
SLOT="${1:-crudeoilm-sats-202610}"
USER_ID="${2:-NVC171}"
TODAY="$(TZ=Asia/Kolkata date +%Y%m%d)"

lease() {
  local key="$1"
  if [[ "$(redis-cli EXISTS "$key")" == "1" ]]; then
    # one field per line; empty fields (e.g. no owner) stay in place
    mapfile -t f < <(redis-cli HMGET "$key" state owner unresolved position last_namespace)
    printf "  %-55s state=%s owner=%s unresolved=%s position=%s last_run=%s\n" \
      "$key" "${f[0]:--}" "${f[1]:--}" "${f[2]:--}" "${f[3]:--}" "${f[4]:--}"
  else
    echo "  $key  (not created yet)"
  fi
}

echo "Account leases (Clean = no run owns the account; Running = a run is live)"
lease "kite-prod:v1:{$SLOT}:lease:$USER_ID"
lease "kite-dev:v1:{$SLOT}:lease:PAPER"

echo
echo "Run ledgers today ($TODAY)"
for prefix in kite-prod kite-dev; do
  redis-cli --scan --pattern "$prefix:v1:{$SLOT}:commands:$TODAY-*" | sort | while read -r key; do
    orders=$(redis-cli HKEYS "$key" | grep -c '^order:' || true)
    review=$(redis-cli HEXISTS "$key" review)
    echo "  $key  orders=$orders$([[ $review == 1 ]] && echo '  (manual review note present)')"
  done
done

echo
echo "Shared order budget"
key="kite-prod:v1:{account-$USER_ID}:order-budget"
if [[ "$(redis-cli EXISTS "$key")" == "1" ]]; then
  echo "  $key  type=$(redis-cli TYPE "$key")"
else
  echo "  $key  (not created yet)"
fi

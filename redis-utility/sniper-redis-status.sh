#!/usr/bin/env bash
# sniper-redis-status.sh v1.0.0
# Read-only view of the Precision Sniper slot's state, in line with
# sats-redis-status.sh: account lease (live and paper), today's run ledgers,
# the shared order budget, plus a summary of today's Sniper JSON logs.
# Never prints or touches the Kite credential keys.
# Usage: ./redis-utility/sniper-redis-status.sh [slot] [kite_user]
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
SLOT="${1:-crudeoilm-sniper-202610}"
USER_ID="${2:-NVC171}"
TODAY="$(TZ=Asia/Kolkata date +%Y%m%d)"
TODAY_DASH="$(TZ=Asia/Kolkata date +%F)"

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

echo "Sniper slot: $SLOT"
echo
echo "Account leases (Clean = no run owns the account; Running = a run is live)"
lease "kite-prod:v1:{$SLOT}:lease:$USER_ID"
lease "kite-prod:v1:{$SLOT}:lease:PAPER"
lease "kite-dev:v1:{$SLOT}:lease:PAPER"

echo
echo "Run ledgers today ($TODAY)"
found=0
for prefix in kite-prod kite-dev; do
  while read -r key; do
    [[ -z "$key" ]] && continue
    found=1
    orders=$(redis-cli HKEYS "$key" | grep -c '^order:' || true)
    review=$(redis-cli HEXISTS "$key" review)
    echo "  $key  orders=$orders$([[ $review == 1 ]] && echo '  (manual review note present)')"
  done < <(redis-cli --scan --pattern "$prefix:v1:{$SLOT}:commands:$TODAY-*" | sort)
done
[[ $found == 0 ]] && echo "  (no runs today)"

echo
echo "Shared order budget"
key="kite-prod:v1:{account-$USER_ID}:order-budget"
if [[ "$(redis-cli EXISTS "$key")" == "1" ]]; then
  echo "  $key  type=$(redis-cli TYPE "$key")"
else
  echo "  $key  (not created yet)"
fi

echo
echo "Today's Sniper logs ($TODAY_DASH)"
for mode in live paper; do
  log="logs/sniper-$mode-$TODAY_DASH.jsonl"
  if [[ ! -f "$log" ]]; then
    echo "  $log  (none)"
    continue
  fi
  python3 - "$log" "$mode" <<'PY'
import json, sys
path, mode = sys.argv[1], sys.argv[2]
counts, fills, last_halt, last_finish = {}, [], None, None
for line in open(path, encoding="utf-8", errors="replace"):
    line = line.strip()
    if not line.startswith("{"):
        continue
    try:
        e = json.loads(line)
    except ValueError:
        continue
    ev = e.get("event", "")
    if not ev.startswith("sniper_"):
        continue
    counts[ev] = counts.get(ev, 0) + 1
    if ev == "sniper_fill":
        fills.append(e)
    elif ev == "sniper_halted":
        last_halt = e.get("reason")
    elif ev == "sniper_node_finished":
        last_finish = e
pos = 0.0
for f in fills:
    q = float(f.get("qty", 0))
    pos += q if f.get("side") == "Buy" else -q
summary = "  ".join(f"{k[7:]}={v}" for k, v in sorted(counts.items()))
print(f"  {path}")
print(f"    events: {summary or '-'}")
print(f"    fills: {len(fills)}  net position from fills: {pos:+.0f} lot")
for f in fills[-5:]:
    print(f"      {f.get('side'):<4} {float(f.get('qty', 0)):.0f} @ {f.get('price')}  {f.get('client_order_id')}")
if last_halt:
    print(f"    last halt: {last_halt}")
if last_finish:
    print(f"    last run finished: fault={last_finish.get('fault')} flat={last_finish.get('flat')}")
PY
done

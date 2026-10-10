#!/usr/bin/env bash
# sniper-redis-status.sh v2.0.0
# Read-only view of the Precision Sniper slot (kite-node 2.21.0+: no journal, no lease):
# whether a run holds the slot's lock file (live and paper), the shared order budget in
# Redis, and a summary of today's Sniper JSON logs.
# Never prints or touches the Kite credential keys.
# Usage: ./redis-utility/sniper-redis-status.sh [slot] [kite_user]
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
SLOT="${1:-crudeoilm-sniper-202610}"
USER_ID="${2:-NVC171}"
TODAY_DASH="$(TZ=Asia/Kolkata date +%F)"
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

echo "Sniper slot: $SLOT"
echo
echo "Instance locks (HELD = a run is active for this slot)"
lock "kite-prod-$SLOT-$USER_ID"
lock "kite-prod-$SLOT-PAPER"
lock "kite-dev-$SLOT-PAPER"

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

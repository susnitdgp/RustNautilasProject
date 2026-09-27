#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CONFIG="config/production-trend-ribbon.json"

python3 - "$CONFIG" <<'PY'
import json, sys
p=sys.argv[1]
v=json.load(open(p))
assert v["strategy"]=="trend_ribbon_boswaves", "wrong strategy selection"
assert v["interval"]=="5minute", "v2.22 candidate must remain five-minute"
assert v["live_orders_enabled"] is False, "SAFETY: live orders must stay disabled"
r=v["trend_ribbon"]
assert r["session"]["reset_daily"] is True, "daily reset must be enabled"
assert r["backtest_square_off"] is True, "historical session square-off must be enabled"
rt=r["realtime"]
assert rt["enabled"] is True, "realtime v2.22 engine must be enabled"
assert rt["pre_close_enabled"] is True
assert rt["pre_close_seconds"]==3
assert rt["fast_hold_seconds"]==2
assert rt["fast_body_atr_min"]==0.5
assert rt["fast_range_atr_min"]==0.75
assert rt["squeeze_exit_enabled"] is True
assert rt["squeeze_bb_length"]==20
assert rt["squeeze_bb_mult"]==2.0
assert rt["squeeze_kc_length"]==20
assert rt["squeeze_kc_mult"]==1.5
assert rt["squeeze_use_true_range"] is True
assert rt["squeeze_weak_bars_required"]==2
assert rt["squeeze_transition_pct"]==70.0
print("config safety: PASS (v2.22 CLEAN, 5m, SQZ ON, live orders OFF)")
PY

cargo fmt --all -- --check
cargo clippy --locked -p kite-node --bin kite-node --tests -- -D warnings
cargo test --locked -p kite-node --bin kite-node
cargo build --locked -p kite-node

BACKTEST_TMP="$(mktemp)"
SIM_TMP="$(mktemp)"
trap 'rm -f "$BACKTEST_TMP" "$SIM_TMP"' EXIT
./target/debug/kite-node native-trend-ribbon-backtest-fixture \
  "$CONFIG" apps/kite-node/tests/fixtures/trend_ribbon_sep18_21_22.json > "$BACKTEST_TMP"
python3 - "$BACKTEST_TMP" <<'PY'
import json, sys
v=json.load(open(sys.argv[1]))
reasons={e["reason"] for e in v["events"]}
assert v["interval"]=="5minute"
assert v["historical_squeeze_exit_enabled"] is True
assert v["closed_trades"] > 10
assert v["open_position"] == 0
assert "squeeze_long_exit" in reasons and "squeeze_short_exit" in reasons
assert "squeeze_re_buy" in reasons or "squeeze_re_short" in reasons
print("historical v2.22 fixture: PASS", {
    "closed_trades":v["closed_trades"],
    "gross_points":v["gross_points"],
})
PY

count=0
for catalog in data/native-catalog/*; do
  [[ -d "$catalog/data/custom/KiteFullTick" ]] || continue
  ./target/debug/kite-node native-trend-ribbon-replay "$CONFIG" "$catalog"
  count=$((count + 1))
done

if [[ "$count" -eq 0 ]]; then
  echo "No KiteFullTick catalogs found on this server; replay stage skipped" >&2
fi

./target/debug/kite-node native-trend-ribbon-sim "$CONFIG" > "$SIM_TMP"
python3 - "$SIM_TMP" <<'PY'
import json, pathlib, sys
lines=pathlib.Path(sys.argv[1]).read_text().splitlines()
complete=None
for line in lines:
    line=line.strip()
    if not line.startswith("{"):
        continue
    try:
        value=json.loads(line)
    except json.JSONDecodeError:
        continue
    if value.get("event")=="trend_ribbon_live_complete":
        complete=value
assert complete is not None, "sandbox LiveNode completion record missing"
assert complete["status"]=="Clean"
assert complete["simulated_feed"] is True
assert complete["live_orders_enabled"] is False
assert complete["broker_orders_sent"] is False
assert complete["open_contracts"] == 0
assert complete["open_orders"] == 0
assert complete["errors"] == []
report=pathlib.Path(complete["report_directory"])
signals=json.load(open(report/"signals.json"))
indicators=json.load(open(report/"indicators.json"))
fast=[s for s in signals if s.get("reason")=="fast_buy"]
assert any(s.get("intent")=="SELL_EXIT" and s.get("target")==1 for s in fast), fast
assert any(s.get("intent")=="BUY" and s.get("target")==1 for s in fast), fast
rt=[i for i in indicators if i.get("realtime_event")=="FAST_BUY"]
assert len(rt)==1, rt
assert rt[0]["snapshot"]["trusted"] is True
print("sandbox LiveNode realtime path: PASS", {
    "signals":complete["signals"],
    "fills":complete["fills"],
    "realtime_event":"FAST_BUY",
    "report":str(report),
})
PY

git diff --check
echo "Trend Ribbon v2.22 CLEAN offline verification: PASS ($count full-tick catalogs replayed)"

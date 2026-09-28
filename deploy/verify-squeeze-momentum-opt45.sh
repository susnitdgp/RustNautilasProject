#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
CONFIG="config/candidate-squeeze-momentum-opt45.json"

python3 - "$CONFIG" <<'PY'
import json,sys
v=json.load(open(sys.argv[1]))
assert v["strategy"]=="squeeze_momentum_lazybear_v2283"
assert v["interval"]=="5minute"
assert v["contracts"]==1
assert v["atr_stop_enabled"] is False
assert v["live_orders_enabled"] is False
s=v["squeeze_momentum"]
expected={
 "sqz_length":20,
 "sqz_length_kc":20,
 "sqz_mult_kc":1.5,
 "sqz_use_true_range":True,
 "entry_strength_bars":2,
 "sqz_weak_bars_req":2,
 "sqz_transition_pct":45.0,
 "session_timezone":"Asia/Kolkata",
 "allow_entries_only_in_session":True,
 "force_flat_at_session_end":True,
 "auto_sq_off_hour":23,
 "auto_sq_off_minute":15,
}
for k,val in expected.items(): assert s[k]==val,(k,s[k],val)
assert s["session"]=={"start":"09:00:00","end":"23:15:00","days":"23456","reset_daily":False}
assert s["display"]=={"show_markers":True,"show_dashboard":True,"shade_outside":True,"outside_session_color":"gray@86"}
print("OPT45 config parity: PASS (2 strength / 2 weak / 45% transition, live orders OFF)")
PY

cargo fmt --all -- --check
cargo clippy --locked -p kite-node --bin kite-node --tests -- -D warnings
cargo test --locked -p kite-node --bin kite-node
cargo build --locked -p kite-node

BACKTEST_TMP="$(mktemp)"
SIM_TMP="$(mktemp)"
trap 'rm -f "$BACKTEST_TMP" "$SIM_TMP"' EXIT

./target/debug/kite-node native-squeeze-momentum-backtest-fixture \
  "$CONFIG" apps/kite-node/tests/fixtures/crude_sep18_21_22.json > "$BACKTEST_TMP"
python3 - "$BACKTEST_TMP" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]))
assert r["interval"]=="5minute"
assert r["force_flat_at_session_end"] is True
assert r["closed_trades"]==27,r["closed_trades"]
assert r["gross_points"]==271.0,r["gross_points"]
assert r["open_position"]==0,r["open_position"]
assert {e["action"] for e in r["events"]}=={"BUY","SELL","SHORT","COVER"}
allowed={"sqz_strength_long","sqz_strength_short","sqz_transition","zero_cross","session_force_flat"}
assert {e["reason"] for e in r["events"]} <= allowed
print("OPT45 historical fixture: PASS",{"closed_trades":r["closed_trades"],"gross_points":r["gross_points"]})
PY

./target/debug/kite-node native-squeeze-momentum-sim "$CONFIG" > "$SIM_TMP"
python3 - "$SIM_TMP" <<'PY'
import json,pathlib,sys
complete=None
for line in pathlib.Path(sys.argv[1]).read_text().splitlines():
    line=line.strip()
    if not line.startswith("{"): continue
    try: v=json.loads(line)
    except json.JSONDecodeError: continue
    if v.get("event")=="squeeze_momentum_live_complete": complete=v
assert complete is not None
assert complete["status"]=="Clean"
assert complete["simulated_feed"] is True
assert complete["live_orders_enabled"] is False
assert complete["broker_orders_sent"] is False
assert complete["open_contracts"]==0
assert complete["open_orders"]==0
assert complete["errors"]==[]
report=pathlib.Path(complete["report_directory"])
signals=json.load(open(report/"signals.json"))
assert signals
assert all(s["intent"] in {"BUY","SELL","SHORT","COVER"} for s in signals)
assert all(s["confirmed_bar_only"] is True for s in signals)
assert all(s["timestamp_ns"] >= s["bar_close_ns"] for s in signals)
assert len({s["bar_close_ns"] for s in signals})==len(signals),"more than one action on a candle"
print("OPT45 sandbox confirmed-bar path: PASS",{"signals":len(signals),"fills":complete["fills"],"report":str(report)})
PY

git diff --check
echo "MCX Crude PURE Squeeze Momentum OPT45 verification: PASS"

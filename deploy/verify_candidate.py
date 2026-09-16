#!/usr/bin/env python3
"""Verify an inactive release against the saved selected-strategy reference."""
import json, subprocess, sys
from pathlib import Path
root=Path(__file__).resolve().parents[1]
candidate=Path(sys.argv[1]).resolve()
binary=candidate/"bin/kite-node"
config=candidate/"config/production-supertrend.json"
reference=root/"backtest_results/supertrend_macd_vwap_2026-08-17_to_2026-09-15_2c404264-451e-4255-8a6a-20d0dbaad733"
check=subprocess.run([str(binary),"native-production-preflight",str(config)],cwd=root,capture_output=True,text=True)
(candidate/"preflight.stdout.log").write_text(check.stdout)
(candidate/"preflight.stderr.log").write_text(check.stderr)
events=[json.loads(line) for line in check.stdout.splitlines() if line.startswith("{")]
assert check.returncode!=0 and len(events)==1
assert events[0]["selection_valid"] and not events[0]["ready_for_live_deployment"]
assert not events[0]["live_orders_enabled"]
report=candidate/"verification_2026-09-15"
run=subprocess.run([str(binary),"native-production-verify",str(config),"2026-09-15",str(reference/"historical_input.json"),str(report)],cwd=root,capture_output=True,text=True)
(candidate/"verification.stdout.log").write_text(run.stdout)
(candidate/"verification.stderr.log").write_text(run.stderr)
assert run.returncode==0,(run.returncode,run.stderr[-2000:])
read=lambda p:json.loads(p.read_text())
summary=read(report/"summary.json")
assert summary["interval"]=="5minute" and summary["entry_confirmation"]
assert summary["open_contracts"]==0 and summary["fills"]==12
assert summary["statistics"]["trades"]==6 and summary["statistics"]["gross_pnl_inr"]==21800
assert read(report/"trades.json")==read(reference/"confirmed/2026-09-15/trades.json")
assert all(f["reason"]!="stop_loss" for f in read(report/"fills.json"))
manifest=read(candidate/"manifest.json")
manifest["verification"]={"preflight_blocks_activation":True,"release_replay_matches_reference_exactly":True,"date":"2026-09-15","trades":6,"fills":12,"gross_pnl_inr":21800,"open_contracts":0,"stop_orders":0}
(candidate/"manifest.json").write_text(json.dumps(manifest,indent=2)+"\n")
print(json.dumps({"candidate":str(candidate),"status":manifest["status"],"verification":manifest["verification"]}))

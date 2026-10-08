# Preliminary ILRC historical gate audit

Run `./target/release/kite-node native-ilrc-gate-audit 145894407` to use the existing read-only Kite 3-minute historical data client for September 1 through October 8, 2026. This standalone audit does not create an execution client, modify strategy thresholds, or interact with Redis.

It calculates the dashboard-style *preliminary* 20-bar sweep/reclaim, close-based break of structure, body displacement, ATR-body displacement and same-session VWAP alignment. It requires 20 current-session prior candles. Some counts refer to independent conditions; they are not a sequential funnel. Signals are not trade intents.

Recorded result: 7,670 bars; 7,130 eligible for inspection; Setup A preliminary sweep/reclaim 615; Setup B BOS 879; body displacement 2,138; ATR displacement 1,956; BOS plus both displacement conditions 523; BOS with VWAP aligned 742; BOS with VWAP rejected 137; BOS plus both displacements and VWAP 444.

**Important limitations:** Does not implement full ILRC A/B pending-state transitions, displacement quality filters outside the dashboard, multi-day warmup, stop/risk/reward admission, internal BOS, retracement, entry timing, position arbitration, brokerage fees or realized broker fills. Counts should not be equated to available trades. Historical rejection auditing of the actual full state machines is the next stage.

# Preliminary ILRC historical gate audit

Run `./target/release/kite-node native-ilrc-gate-audit 145894407` to use the existing read-only Kite 3-minute historical data client for September 1 through October 8, 2026. This standalone audit does not create an execution client, modify strategy thresholds, or interact with Redis.

It calculates the dashboard-style *preliminary* 20-bar sweep/reclaim, close-based break of structure, body displacement, ATR-body displacement and same-session VWAP alignment. It requires 20 current-session prior candles. Some counts refer to independent conditions; they are not a sequential funnel. Signals are not trade intents.

Recorded result: 7,670 bars; 7,130 eligible for inspection; Setup A preliminary sweep/reclaim 615; Setup B BOS 879; body displacement 2,138; ATR displacement 1,956; BOS plus both displacement conditions 523; BOS with VWAP aligned 742; BOS with VWAP rejected 137; BOS plus both displacements and VWAP 444.

**Important limitations:** Does not implement full ILRC A/B pending-state transitions, displacement quality filters outside the dashboard, multi-day warmup, stop/risk/reward admission, internal BOS, retracement, entry timing, position arbitration, brokerage fees or realized broker fills. Counts should not be equated to available trades. Historical rejection auditing of the actual full state machines is the next stage.

## Full historical entry-event comparison

The same command now additionally invokes the existing ILRC Setup A and Setup B entry-event functions with the configured October 2026 instrument and strategy parameters. For September 1 through October 8, 2026, the historical functions reported 8 A and 121 B entry events. October 8 alone had 2 B model events. These are historical signal-engine outputs; **neither broker admission nor executable fills are asserted**. The preliminary counts use dashboard-style bar conditions and must not be interpreted as sequential rejections from the full signal engines. In particular, event time and historical theoretical entry-price assumptions may differ from the live order path. Investigate run start time, accepted candles, position/lease state, event timestamps and broker-order lifecycle before attributing October 8 zero trades to VWAP alone.

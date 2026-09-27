# BOSWaves Trend Ribbon v2.23 Exit-First — Rust / Nautilus Port

The active strategy is **Trend Ribbon [BOSWaves] - FAST + SQZ Exit + Reentry Exit-First v2.23**. `config/production-trend-ribbon.json` is the source of truth for the selected candle interval and all strategy inputs used by the Rust engine.

## Core Trend Ribbon

The default trend inputs match Pine: ALMA(34, 0.85, 6.0), population standard deviation length 34 with confirmation multiplier 0.65, ATR(14), and a 3-bar ALMA slope normalized by ATR with minimum absolute score 0.08.

A bullish flip requires `slope_score > +0.08` and close above `ALMA + stdev * 0.65`. A bearish flip requires `slope_score < -0.08` and close below `ALMA - stdev * 0.65`. Confirmed trend direction resets at the session boundary while indicator history remains continuous.

## Squeeze Momentum v2.23 exit

The active exit engine uses the LazyBear-style Squeeze Momentum calculation from the supplied Pine v2.23 source.

All Squeeze inputs are JSON-configurable under `trend_ribbon.realtime`:

- `squeeze_exit_enabled` (default `true`)
- `squeeze_bb_length` (20)
- `squeeze_bb_mult` (2.0; retained for Pine input parity)
- `squeeze_kc_length` (20)
- `squeeze_kc_mult` (1.5)
- `squeeze_use_true_range` (`true`)
- `squeeze_weak_bars_required` (2)
- `squeeze_transition_pct` (70.0)

To match the supplied Pine source exactly, the BB deviation calculation intentionally uses `squeeze_kc_mult`, not `squeeze_bb_mult`.

LONG arms when Squeeze momentum is positive and strengthening. SHORT arms when it is negative and strengthening. After arming, the engine tracks the strongest momentum peak/trough. A normal first weakening bar is only a warning.

Exit requires both the configured consecutive weakening-bar count and configured retracement percentage toward zero. Crossing the zero line exits immediately.

Only one Squeeze exit is allowed per confirmed ribbon trend. After a Squeeze exit the strategy is flat and blocks ordinary same-trend close synchronization.

One same-trend continuation re-entry is allowed when the confirmed ribbon trend is unchanged and Squeeze momentum strengthens for two consecutive bars (`RB` for LONG, `RS` for SHORT). The one-exit latch remains set after that re-entry and is reset only when confirmed ribbon direction changes or the session resets.

## Realtime priority

The forming five-minute candle is previewed from completed history plus Kite LTP. Realtime event priority is:

1. Squeeze Momentum exit
2. Squeeze same-trend `RB` / `RS` re-entry
3. FAST reversal
4. pre-close reversal
5. confirmed-close Trend Ribbon synchronization

FAST uses a 2-second opposite-condition hold plus body/ATR >= 0.50 or range/ATR >= 0.75. Pre-close uses the final 3 seconds and therefore still requires a market tick in that window. Maximum one realtime strategy event is admitted per candle.

## Session and safety

Trading session is 09:00–23:15 Asia/Kolkata with daily trend reset and strategy square-off. The 23:10–23:15 candle remains an in-session candle. `live_orders_enabled` remains `false` in the committed candidate.

## Historical and dashboard commands

Confirmed-bar historical replay:

`./target/debug/kite-node native-trend-ribbon-backtest-fixture config/production-trend-ribbon.json apps/kite-node/tests/fixtures/trend_ribbon_sep18_21_22.json`

Single-day interactive dashboard:

`./target/debug/kite-node native-trend-ribbon-dashboard-history config/production-trend-ribbon.json 2026-09-22`

Single-day deterministic snapshot:

`./target/debug/kite-node native-trend-ribbon-dashboard-snapshot config/production-trend-ribbon.json 2026-09-22`

Date-range summary:

`./target/debug/kite-node native-trend-ribbon-dashboard-summary config/production-trend-ribbon.json 2026-09-22 2026-09-25`

Date-range snapshot:

`./target/debug/kite-node native-trend-ribbon-dashboard-summary-snapshot config/production-trend-ribbon.json 2026-09-22 2026-09-25`

Live monitoring with Kite market data and Nautilus Sandbox only:

`./target/debug/kite-node native-trend-ribbon-dashboard-live config/production-trend-ribbon.json 3600`

The dashboard shows Ribbon values, FAST/pre-close state, Squeeze value and state, arm/extreme, weak-bar count, transition percentage, exit/re-entry readiness, plus current-trade MFE, MAE, giveback and retained-profit percentage.

## Validation

Run:

`bash deploy/verify-trend-ribbon-v223.sh`

Exact TradingView realtime parity still requires a current-contract full-tick recording and side-by-side Pine/Rust event timestamps. Confirmed five-minute historical replay is deterministic but cannot reconstruct intrabar FAST/pre-close timing exactly.

- `squeeze_reentry_enabled=false` disables same-trend SQZ RB/RS re-entry after QLX/QSX.

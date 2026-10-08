# Preliminary ILRC historical gate audit

Run `./target/release/kite-node native-ilrc-gate-audit 145894407` to use the existing read-only Kite 3-minute historical data client for September 1 through October 8, 2026. This standalone audit does not create an execution client, modify strategy thresholds, or interact with Redis.

It calculates the dashboard-style *preliminary* 20-bar sweep/reclaim, close-based break of structure, body displacement, ATR-body displacement and same-session VWAP alignment. It requires 20 current-session prior candles. Some counts refer to independent conditions; they are not a sequential funnel. Signals are not trade intents.

Recorded result: 7,670 bars; 7,130 eligible for inspection; Setup A preliminary sweep/reclaim 615; Setup B BOS 879; body displacement 2,138; ATR displacement 1,956; BOS plus both displacement conditions 523; BOS with VWAP aligned 742; BOS with VWAP rejected 137; BOS plus both displacements and VWAP 444.

**Important limitations:** Does not implement full ILRC A/B pending-state transitions, displacement quality filters outside the dashboard, multi-day warmup, stop/risk/reward admission, internal BOS, retracement, entry timing, position arbitration, brokerage fees or realized broker fills. Counts should not be equated to available trades. Historical rejection auditing of the actual full state machines is the next stage.

## Full historical entry-event comparison

The same command now additionally invokes the existing ILRC Setup A and Setup B entry-event functions with the configured October 2026 instrument and strategy parameters. For September 1 through October 8, 2026, the historical functions reported 8 A and 121 B entry events. October 8 alone had 2 B model events. These are historical signal-engine outputs; **neither broker admission nor executable fills are asserted**. The preliminary counts use dashboard-style bar conditions and must not be interpreted as sequential rejections from the full signal engines. In particular, event time and historical theoretical entry-price assumptions may differ from the live order path. Investigate run start time, accepted candles, position/lease state, event timestamps and broker-order lifecycle before attributing October 8 zero trades to VWAP alone.

## October 8 live-admission investigation

The `oct8_events` field now exposes the actual historical entry-event timestamps, separately from `entry_time` (historical modeled reference). Setup A: zero. Setup B: two LONG events observed at **2026-10-08 11:06 IST** and **11:45 IST**, with modeled historical entry references at 11:03 and 11:42. Neither event is a verified real broker order.

The production actor (`ilrc_live_actor.rs`) only accepts events when `observed_at` equals its most recently completed candle close. It also ignores historical warmup candles and the first live anchor candle, and blocks entries with pending orders/positions, stopping/faulted control state, or other risk/lifecycle guards. The 23:15 session cutoff does not explain these earlier timestamps. Compare the *actual production launch time*, first live anchor timestamp, warmup continuity, actor admission log, and Kite broker order history before drawing any conclusion about zero live fills. The separate production instance is not available from the development box for log inspection; nothing was restarted or changed in production.

## October 8 replay constrained to actual live launch

The reported production launch was 15:30 IST. Historical events with `observed_at` strictly later than 15:33 IST (a conservative allowance for initial 3-minute live-candle anchoring), and strictly before the 23:15 IST entry cutoff, numbered **zero Setup A and zero Setup B**. The two Setup B events observed at 11:06 and 11:45 were before launch. The audit now emits `oct8_after_launch_and_anchor`. The comparison is conditional on the reported launch time and assumes feed continuity; it cannot independently verify the exact anchor time or production order/broker state. No evidence of missed post-launch historical strategy signals on this date.

## Multi-session next-bar-open execution mock (September 1–October 8)

The audit also reuses `ilrc_timed_mock::research_summary` (0.5 adverse points per execution side), preserving the real historical A and B entry-event functions and one-position-at-a-time mock policy. Input: 7,670 consecutive 3-minute futures candles. Setup A: 8 candidates, 7 fills, 7 exits, 2 stops, 3 targets, 2 breakeven, +243.968 gross points after assumed slippage. Setup B: 121 candidates, 118 fills/exits, 58 stops, 50 targets, 10 breakeven, +127.011 gross points. Combined: 129 candidates, 124 fills/exits, 1 blocked, 4 unfilled, 60 stops, 52 targets, 12 breakeven, +233.534 gross points. These are model outputs, not executable profits.

**Material limitations:** The reusable timed mock has no daily EOD position close, full Kite tax/charge model, partial fills, exchange queue or bar-internal event sequencing. Positions can remain open across sessions. The simulated point returns cannot responsibly be called *net returns* or ascribed to a particular session. A session-flat, charges-inclusive, instrument-lot-verified independent replay is required before any strategy change or live performance conclusion. No live trades or portfolio state changes occurred.

## Daily-flat 2-point round-trip net research

A separate research-only daily-session replay reuses the timed mock with 0.5 points adverse slippage on each execution side, user-estimated 2 points fixed cost per completed round trip, independent daily position state and an assumed liquidation at the last bar ending no later than 23:15 IST. Output includes daily points and daily-close realized equity drawdown. For 27 observed historical sessions between 2026-09-01 and 2026-10-08: Setup A 7 completed, zero EOD liquidations, +229.968 net points, 34.532 daily-close drawdown; Setup B 118 completed, one EOD liquidation, +18.011 net points, 487.580 drawdown; combined 124 completed, one EOD liquidation, +112.534 net points, 469.511 drawdown.

**Important:** The one forced exit is priced at the historical 23:12 bar close (23:15 clock time), not actual Kite tradability or fill data. A single forced exit meaningfully changes results. Backtest remains exploratory, with unverified intrabar fills and no quote latency, live position management or liquidity. Neither point net results nor daily-close drawdowns establish a deployable edge. Previous gross-only mock results remain preserved separately for comparison. No production changes.

## Validation of session-end trade and concentration, 2026-10-09

All calculations here are research-only 3-minute candle fills with 0.5 adverse points per side and 2 points per round trip, over 27 sessions (September 1–October 8). Isolated the **single modeled EOD exit** on September 28: Setup B SHORT signal at 22:54 IST, entry 8924.5, liquidation at 8913.5 on the 23:12 opening-time candle (nominally 23:15 close), +9.0 net points after both slippage and round-trip charges. Exit is **not verified executable**. The simulator now exposes `eod_details`, `eod_trade_net_points` and `net_points_excluding_eod_trades` to make the assumption explicit.

Removing only that EOD trade: A +229.968 (unchanged), B +9.011, Combined +103.534. Removing **all September 28 model results**: A +73.968, B -190.144, Combined -116.176. The September 28 combined daily +228.710 dominates the overall +112.534; the result is **not robust to excluding that session**.

Setup A's seven mock trades span six dates: September 1 +64.0; September 2 two trades totaling -19.299; September 3 -15.233; September 18 +47.0; September 24 -2.5; September 28 +156.0. The September 28 single trade contributes roughly 68% of all A net points. Seven trades are insufficient to claim statistical significance.

**Unresolved limitations before live conclusion:** Test the modeled EOD exit against quote/tick history and actual cutoffs, enforce next-open fill chronology and tick rounding, validate no same-bar stop exposure, compare market depth and tick spreads, account for order acknowledgements and broker reconciliation, and run true out-of-sample monthly contracts after rollover. No changes to live ILRC execution, production Redis or production process.

## Chronological stability check (not untouched holdout)

The audit now splits daily-flat, 0.5-point adverse slippage per side, 2-point round-trip cost results into **September 1–30 (22 sessions)** and **October 1–8 (5 sessions)** without re-optimizing strategy parameters. September: Setup A 7 trades +229.968 points; Setup B 99 trades **-220.028** points; Combined 105 trades **-125.505** points. October: Setup A no trades; Setup B 19 trades +238.040 points; Combined 19 trades +238.040 points. This is marked a **chronological stability diagnostic**, not a genuine untouched out-of-sample validation, because October outcomes have already been inspected in previous ILRC research. Profitability appears concentrated in a small number of days, and no repeatable edge is established. No live parameters were changed.

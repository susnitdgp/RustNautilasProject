# BOSWaves Trend Ribbon — Rust / Nautilus Port

This module ports the supplied TradingView Pine v6 Trend Ribbon signal logic into the native Rust/Nautilus execution path with a JSON-selected three- or five-minute candle interval.


## JSON-selected candle interval

`config/production-trend-ribbon.json` is the interval source of truth. The current v2.10 candidate selects `5minute`. The selection drives the Kite historical endpoint, candle alignment, close timestamps, Nautilus external bar type, freshness checks, recovery replay, report metadata and terminal countdown.

Changing the timeframe changes the economic meaning of bar-count parameters. On three-minute candles, ALMA(34) spans 102 minutes and the three-bar slope comparison spans nine minutes; on five-minute candles they span 170 and fifteen minutes respectively. Parameters are not automatically rescaled.

A running process retains the interval loaded at startup. Editing JSON or building another binary does not change an already running session; stop cleanly, verify the broker is flat, then restart to activate the new interval.

## Signal parity

The Rust engine uses the Pine defaults: ALMA length 34, offset 0.85, sigma 6.0; population standard deviation length 34 with 0.65 multiplier; ATR(14) using Pine/Wilder RMA semantics; and a 3-bar ALMA slope normalized by ATR with a minimum absolute score of 0.08.

A bullish flip requires slopeScore > 0.08 and close > ALMA + stdev * 0.65. A bearish flip requires slopeScore < -0.08 and close < ALMA - stdev * 0.65. The state persists until the opposite setup occurs. Entries are emitted only on fresh flips, not from historical warmup state.

The terminal `Transition` row separates persistent direction from a fresh flip. With no post-start flip it reports that the current direction was inherited from warmup or rebuild. A flip reports previous and new direction, completed-candle time, raw `+1`/`-1` signal, close-versus-band trigger, normalized slope, and whether the displayed observation is `LIVE` or `REBUILT`. The `Last signal` row remains the source for actual order intent and pending execution state.

The visual ribbon, candle coloring, labels, and alert text are chart presentation features and are intentionally not part of the execution engine.

## v2.10 realtime engine

The current candidate retains completed Kite historical bars as the authoritative confirmed-bar and recovery source, while subscribing to native `KiteFullTick` data for the forming candle. The realtime candle uses Kite LTP, not bid/ask midpoint. ALMA, deviation, ATR and WaveTrend are previewed from the last confirmed state plus the mutable current candle; every tick is not committed as a new EMA/ATR bar.

Realtime event priority normally matches the Pine design: WaveTrend exit first, FAST reversal second, pre-close reversal third, then the confirmed completed-bar Trend Ribbon transition. WaveTrend uses the dynamic ATR-regime arm, best WT1 peak/trough tracking, five-point pullback/rebound and tick-to-tick WT1 slope. FAST requires a two-second opposite setup hold plus body/ATR or range/ATR strength. Pre-close uses the final three seconds of the selected candle.

An experimental Trend Weakness exit is implemented but intentionally disabled in production. When enabled, priority becomes WaveTrend exit, Trend Weakness exit, FAST reversal, pre-close reversal, then confirmed Trend Ribbon transition. The weakness exit goes FLAT only and applies the same-trend flat lock used by WaveTrend exits. The current safe candidate requires price beyond ALMA by 0.10 ATR, opposite normalized slope at least the full configured reversal slope magnitude (factor 1.0), and a two-second realtime hold. Historical candle testing uses the same price/slope condition on confirmed five-minute bars but cannot reproduce the two-second intrabar hold, so it is only a proxy for realtime behavior.

Only one strategy event is admitted per candle. Reversal targets use the existing Nautilus/Kite reducing-then-entering order path. The causal trigger is retained across both legs of a split reversal. A WaveTrend exit leaves the target flat and blocks same-trend confirmed-close synchronization until the confirmed direction changes. FAST/pre-close reversals are reconciled back to the persistent confirmed direction on a later confirmed bar if the intrabar reversal does not survive confirmation.

The first partial candle after startup is deliberately not trusted because its true intrabar open/high/low cannot be reconstructed from a mid-candle WebSocket subscription. After a new boundary, realtime decisions remain gated until the canonical prior completed bar has arrived. Feed recovery rebuilds both confirmed and realtime indicator state before realtime signals resume.

## Session and execution

The Pine trading session is 09:00–23:15 Asia/Kolkata. The v2.10 candidate enables daily Ribbon reset and evaluates the Pine session on the candle open, so the 23:10–23:15 five-minute candle remains an in-session bar. The existing production safety policy still caps the effective live cutoff using the MCX calendar. Ribbon also uses the quote-independent 250 ms strategy square-off clock callback rather than relying on another market tick to flatten at the application cutoff.

Feed recovery rebuilds the indicator from validated completed bars. Rebuilds never replay historical entries. An existing position is retained only if the rebuilt current direction still agrees with it; otherwise the strategy flattens.

## Files and commands

Candidate selection: `config/production-trend-ribbon.json`; its `interval` field selects `5minute` and `live_orders_enabled` is intentionally `false` while v2.10 is qualified.

Read-only full-tick capture for realtime parity work uses no strategy or execution client. On an open market session, record for an explicit duration with:
`./deploy/record-trend-ribbon-ticks.sh 3600`
The command writes a new `data/native-catalog/<UUID>/` with complete `KiteFullTick` packets and verifies the Parquet round-trip. It requires market-data credentials but cannot submit orders. Replay a saved capture with:
`./target/debug/kite-node native-trend-ribbon-replay config/production-trend-ribbon.json data/native-catalog/<UUID>`

Recorded full-tick codec/forming-candle replay (never accesses the broker):
`./target/debug/kite-node native-trend-ribbon-replay config/production-trend-ribbon.json data/native-catalog/<RUN_UUID>`

Historical v2.10 confirmed-bar backtest against a candle fixture:
`./target/debug/kite-node native-trend-ribbon-backtest-fixture config/production-trend-ribbon.json apps/kite-node/tests/fixtures/trend_ribbon_sep18_21_22.json`

Repeatable offline qualification:
`bash deploy/verify-trend-ribbon-v210.sh`

Production configuration check:
`./target/release/kite-node native-trend-ribbon-production-check config/production-trend-ribbon.json config/kite-production.json`

Manual production launcher:
`./deploy/run-trend-ribbon-live.sh`

The launcher can place real orders. It must never be started merely as a build or validation step.

## Remaining parity validation

The current strict suite covers deterministic rebuilds, session/reset semantics, confirmed-close synchronization, FAST, pre-close, WaveTrend exit priority/flat-lock behavior, dynamic WT math, packet replay and the existing reviewed Sep 18/21/22 confirmed-flip regression. Native Kite mock runs have separately exercised a clean FAST split reversal (SHORT exit fill before LONG entry), isolated WT short/long exits with same-trend flat lock, and isolated pre-close reversal; all validation runs kept real orders disabled.

The historical v2.10 simulator now mirrors confirmed-bar Trend Ribbon entries/reversals, dynamic WaveTrend arm/pullback exits and session square-off. On the bundled 602-bar October-contract fixture it currently reports 22 closed trades, 12 winners, 10 losers and +187 gross points before costs. That is a deterministic Rust result, not an independent TradingView WT-exit reference.

Exact TradingView realtime parity still requires an actual full-session CRUDEOIL tick recording for the current contract and side-by-side Pine/Rust event timestamps. The short native catalogs in this repository validate decoding and forming-candle mechanics but use the older token and are not treated as October parity evidence. Real broker fill latency/slippage and failure-mode qualification also remain outstanding.

Completed-bar publication remains boundary-aligned and guarded against incomplete, revised or gapped history before strategy state is rebuilt.

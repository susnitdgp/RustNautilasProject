# BOSWaves Trend Ribbon — Rust / Nautilus Port

This module ports the supplied TradingView Pine v6 Trend Ribbon signal logic into the native Rust/Nautilus execution path with a JSON-selected three- or five-minute candle interval.


## JSON-selected candle interval

`config/production-trend-ribbon.json` is the interval source of truth. Supported Trend Ribbon values are `3minute` and `5minute`; the active production selection currently uses `5minute`. The selection drives the Kite historical endpoint, candle alignment, close timestamps, Nautilus external bar type, freshness checks, recovery replay, report metadata and terminal countdown. Pivot Point and the original Supertrend/MACD/VWAP selection remain restricted to five-minute candles.

Changing the timeframe changes the economic meaning of bar-count parameters. On three-minute candles, ALMA(34) spans 102 minutes and the three-bar slope comparison spans nine minutes; on five-minute candles they span 170 and fifteen minutes respectively. Parameters are not automatically rescaled.

A running process retains the interval loaded at startup. Editing JSON or building another binary does not change an already running session; stop cleanly, verify the broker is flat, then restart to activate the new interval.

## Signal parity

The Rust engine uses the Pine defaults: ALMA length 34, offset 0.85, sigma 6.0; population standard deviation length 34 with 0.65 multiplier; ATR(14) using Pine/Wilder RMA semantics; and a 3-bar ALMA slope normalized by ATR with a minimum absolute score of 0.08.

A bullish flip requires slopeScore > 0.08 and close > ALMA + stdev * 0.65. A bearish flip requires slopeScore < -0.08 and close < ALMA - stdev * 0.65. The state persists until the opposite setup occurs. Entries are emitted only on fresh flips, not from historical warmup state.

The terminal `Transition` row separates persistent direction from a fresh flip. With no post-start flip it reports that the current direction was inherited from warmup or rebuild. A flip reports previous and new direction, completed-candle time, raw `+1`/`-1` signal, close-versus-band trigger, normalized slope, and whether the displayed observation is `LIVE` or `REBUILT`. The `Last signal` row remains the source for actual order intent and pending execution state.

The visual ribbon, candle coloring, labels, and alert text are chart presentation features and are intentionally not part of the execution engine.

## Session and execution

The Pine trading session is 09:00–23:15 Asia/Kolkata. Paper configuration preserves that cutoff. Production retains the repository safety policy: the MCX calendar closes at 23:30 and the existing 30-minute production exit buffer makes the effective live cutoff 23:00 IST.

Feed recovery rebuilds the indicator from validated completed bars. Rebuilds never replay historical entries. An existing position is retained only if the rebuilt current direction still agrees with it; otherwise the strategy flattens.

## Files and commands

The archived paper/simulation selection remains five-minute: `config/backup/trend-ribbon-boswaves.json`.

Live-capable selection: `config/production-trend-ribbon.json`; its `interval` field currently selects `5minute`.

Offline simulation:
`./target/debug/kite-node native-trend-ribbon-sim config/backup/trend-ribbon-boswaves.json`

Production configuration check:
`./target/release/kite-node native-trend-ribbon-production-check config/production-trend-ribbon.json config/kite-production.json`

Manual production launcher:
`./deploy/run-trend-ribbon-live.sh`

The launcher can place real orders. It must never be started merely as a build or validation step.

## Remaining parity validation

Unit tests verify deterministic rebuilds, session gating, parameter validation, and bidirectional flips. The synthetic Nautilus LiveNode simulation has also been exercised.

Exact TradingView parity still requires replaying the same CRUDEOIL OHLC candles at the selected interval through Pine and Rust and comparing ALMA, deviation, ATR, slope score, direction, and flip timestamps bar by bar. One supervised real long entry and graceful reducing exit have been observed; broader broker-fill, outage and full-session behavior remain unverified.

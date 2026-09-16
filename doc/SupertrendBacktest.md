# Supertrend five-minute native backtest

## Run

From /home/ubuntu/RustNautilasProject:

    cargo run --locked -p kite-node -- native-supertrend-backtest 2026-09-15

This downloads the MCX instrument master, resolves the pinned CRUDEOIL26SEPFUT
contract and reads Kite historical candles using the existing market-data
credentials in Redis. It performs no broker order submission.
Seven calendar days before the requested day are requested for warmup.

Replay the exact saved input without an HTTP request:

    cargo run --locked -p kite-node -- native-supertrend-backtest 2026-09-15 backtest_results/RUN_DIRECTORY/candles.json

Replace RUN_DIRECTORY with the desired completed run folder.
Each invocation creates a new, immutable UUID-named directory; it never overwrites
an earlier result. Input JSON must identify the pinned instrument/token and
5minute interval. Missing/duplicate/unordered candles and invalid OHLC are rejected.

## Strategy and execution assumptions

- Five-minute candles; Supertrend multiplier 2 and ATR period 7.
- ATR is Nautilus AverageTrueRange with MovingAverageType::Wilder.
- Nautilus Wilder seeds from the first true range, not an initial seven-value SMA.
  At least 100 prior-session candles are required; the actual run used 774.
- Supertrend bands use HL2 plus/minus 2 ATR, carried bands and prior close.
  Initial trend at ATR readiness is bearish. Only strict band crossings flip it.
- The first session open adopts the warmed-up trend. Later flips reverse exposure.
- Completed bars update the indicator; execution is at the next candle open.
  OHLC candles are supplied to the strategy only at their closing timestamps.
- Native simulated market orders use synthetic zero-spread quotes at the next open.
  These quotes are execution assumptions, not recorded bid/ask observations.
- Reversals use a reducing exit followed by a separate entry after its fill.
  Exposure is one standard contract (100 barrels), either long or short.
- The final position is closed at the last candle's close, at 23:30 IST.
- This September contract runner expects 174 bars from 09:00 through 23:25 IST.
  Short sessions or incomplete data fail explicitly instead of yielding a partial result.
- Fees, spread, slippage, liquidity and market impact are excluded.
  Results are gross simulated P&L, not net realizable returns.
- The existing crossover strategy's additional stop-loss/target overlay is not used.
- Raw Nautilus risk/return statistics include the warmup period; annualized metrics
  from this single-session run should not be used to assess profitability.
- Order/application state stays in the native Redis cache. Files are reports/input
  snapshots, not an alternative state database. Real-order gates are unchanged.

## Files

| Component | Source |
|---|---|
| Native ATR and Supertrend bands | ../apps/kite-node/src/native_node/supertrend.rs |
| Native strategy callbacks and simulated orders | ../apps/kite-node/src/native_node/supertrend_actor.rs |
| Input validation and replay timestamps | ../apps/kite-node/src/native_node/supertrend_input.rs |
| BacktestNode setup and results | ../apps/kite-node/src/native_node/supertrend_backtest.rs |
| Historical market-data reader | ../crates/kite-adapter/src/http/historical.rs |
| Shared result writer | ../apps/kite-node/src/native_node/backtest_report.rs |

Each successful Supertrend folder contains README.md, summary.json, candles.json,
indicators.json, signals.json and fills.json. Failure folders contain failure.json
and any diagnostics written before the failure; they have no completed P&L report.
The older native-backtest command now also saves summary.json in backtest_results.
Result folders are ignored by Git and retained on the server.

## Verified historical run

15 September 2026: 174 session bars, 774 warmup bars, 16 fills, 8 completed trades,
zero remaining position. Gross P&L INR 32,300; starting balance INR 1,000,000;
ending balance INR 1,032,300. Fill-pair arithmetic independently agrees with Nautilus.

Result folder:

    backtest_results/supertrend_7_2_5minute_2026-09-15_5cc5c456-889c-4299-b945-68d6dffe8121/

Source: https://kite.trade/docs/connect/v3/historical/

Verification: 181 workspace tests passed, zero failed, one existing ignored fixture.
Clippy with warnings denied, rustfmt and git diff checks passed. The native-engine
Supertrend test runs in an isolated child process because Nautilus endpoints are
process-global. It checks all four intents, next-open fill prices and final flatness.

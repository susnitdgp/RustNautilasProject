# VWAP + EMA crossover + MACD seven-session backtest

Run from /home/ubuntu/RustNautilasProject:

    cargo run --locked -p kite-node -- native-vwap-backtest 2026-09-15

This selects seven trading sessions ending on the supplied date: for this run,
7, 8, 9, 10, 11, 14 and 15 September 2026. Thirty calendar days of historical data
are fetched for indicator warmup and the requested sessions. All target sessions
must pass completeness checks before any native backtest is run.

To reproduce from saved input without an HTTP request:

    cargo run --locked -p kite-node -- native-vwap-backtest 2026-09-15 backtest_results/RUN_DIRECTORY/historical_input.json

## Rules

- Instrument: CRUDEOIL26SEPFUT.MCX; five-minute OHLCV candles.
- Long: EMA 9 freshly crosses above EMA 21, candle close is strictly above
  session VWAP, and MACD is strictly above its signal line on that same closed bar.
- Short: EMA 9 freshly crosses below EMA 21, close is strictly below session VWAP,
  and MACD is strictly below its signal line on that same closed bar.
- MACD parameters are 12/26, with a 9-period EMA signal line. No additional
  MACD zero-line filter or simultaneous MACD crossover is required.
- VWAP uses volume-weighted HLC3 from bars and resets for each IST session.
  It approximates VWAP from OHLCV; it is not a tick-by-tick VWAP.
- Indicators are native Nautilus EMA, VWAP, MACD and AverageTrueRange.
- ATR period is 14 with Wilder smoothing, warmed up with prior sessions.
- Market entry is simulated at the next candle open, not the signal candle close.
- Size is one standard contract, multiplier 100 barrels.
- Fixed stop-market order at 1.5 times the signal-bar ATR from the filled entry.
  Stop prices round outward to the instrument's whole-rupee tick.
- Stop fills use the native OHLC matching engine; intrabar timing is synthetic.
- A fully confirmed opposite setup closes the current position and then opens
  the opposite side in a separate order. A crossover failing either confirmation
  does not close the position.
- After a stop, no automatic re-entry: another fresh qualifying crossover is needed.
- Positions and remaining stop orders are closed/cancelled at session end.
- No trailing stop, profit target, fee, bid/ask spread, slippage or impact model.
  P&L is gross simulated P&L. The same fixed size is used each day; no compounding.
- Indicators update only at bar completion. Stops are placed only after entry fills.

## Calendar

The runner is scoped to September 1–21, 2026 for the pinned contract.
Regular sessions are 09:00–23:30 IST (174 bars). September 14 was an evening-only
session, 17:00–23:30 IST (78 bars). Weekends are excluded. Data gaps fail explicitly.

Sources:
- https://www.mcxindia.com/market-operations/trading-surveillance/trading-holidays
- https://zerodha.com/marketintel/bulletin/457567/trading-holiday-on-account-of-ganesh-chaturthi-on-september-14-2026

## Review locations

| Component | File under apps/kite-node/src/native_node |
|---|---|
| Native indicators and entry rules | vwap_signal.rs |
| Native strategy, entries, exits and stop orders | vwap_actor.rs |
| Calendar, validation and chronological replay | vwap_input.rs |
| Single-session BacktestNode and Redis cache | vwap_backtest.rs |
| Seven-session runner and combined report | vwap_batch.rs |
| Independent fill-pair P&L and statistics | vwap_report.rs |

Reports are retained under backtest_results/vwap_ema_macd_5minute_7sessions_*.
The root has summary.json, README.md, historical_input.json and trades.json.
Each date directory has candles.json, indicators.json, signals.json, fills.json,
trades.json and summary.json. Logs and failure.json preserve failed-run evidence.
Order/application state uses Redis; report files do not replace Redis persistence.

The combined drawdown metric measures closed-trade equity only, not intratrade
mark-to-market drawdown. Raw native annualized statistics include warmup and
should not be interpreted as reliable strategy-quality measures.
Real broker execution remains disabled; only historical market data is requested.

## Historical result: 7–15 September 2026

Seven sessions, 1,122 target bars, nine trades (five long, four short), four winners.
Gross P&L: INR 97,700. Win rate: 44.44%. Profit factor: 6.5829.
Maximum closed-trade equity drawdown: INR 13,800. All sessions ended flat.
The fill audit verified next-open entries, qualifying indicator values, all three
stop fills and agreement between fill-pair arithmetic and native daily P&L.
A separate synthetic gap test verified the worse opening price when a stop gaps.

| Date | Trades | Gross P&L INR |
|---|---:|---:|
| 2026-09-07 | 2 | -2,600 |
| 2026-09-08 | 0 | 0 |
| 2026-09-09 | 1 | 17,600 |
| 2026-09-10 | 2 | 59,000 |
| 2026-09-11 | 1 | 18,300 |
| 2026-09-14 | 1 | 19,200 |
| 2026-09-15 | 2 | -13,800 |

Result folder:

    backtest_results/vwap_ema_macd_5minute_7sessions_2026-09-15_bfd3c424-d640-4346-9cda-f4f63f40e1cc/

Verification: 186 workspace tests passed, zero failed, one existing ignored fixture.
Clippy with warnings denied, rustfmt and diff checks passed. Tests cover signal
confirmation, session VWAP reset, holiday coverage, native long/short stops,
flat shutdown and worse-price fills when the market gaps through a stop.

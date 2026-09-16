# Supertrend + MACD + VWAP review

## Rules fixed before the run

- CRUDEOIL26SEPFUT.MCX; five-minute candles; one standard lot (100 barrels).
- Supertrend period 7, multiplier 2: custom bands over native Nautilus Wilder ATR.
- Native MACD EMA12 minus EMA26; native EMA9 of MACD as its signal.
- Native session VWAP uses HLC3 and candle volume, resetting each IST session.
- Long: bullish Supertrend, close strictly above VWAP, MACD strictly above signal.
- Short: bearish Supertrend, close strictly below VWAP, MACD strictly below signal.
- No MACD zero-line requirement and no EMA9/21 crossover.
- When flat, entry may occur when confirmation arrives later in the same trend;
  it does not require a fresh Supertrend flip. Use only completed candle values.
- Do not enter using the prior session's VWAP at the new session open.
- Hold until a completed-bar Supertrend reversal or session close. Loss of
  MACD/VWAP confirmation alone does not exit. Confirmation never blocks an exit.
- Separate reducing exit and new one-lot entry on reversal if confirmed.
- Next-open market fills; final session exit at last close.
- No separate 1.5 ATR stop or target. This preserves the old Supertrend exits.
- Fees, spread, slippage and liquidity constraints excluded.

## Same thirty-calendar-day data

17 August–15 September 2026, 22 sessions, 3,732 evaluated bars.
870 prior candles (10–14 August) warm up indicators. Same September contract
throughout; no automatic roll. September 14 is evening only.
The input is byte-identical to the preceding 30-day EMA/VWAP comparison:

    f05c41cdb8284150918a087159ae37ddc0957c01e855b41cdf9d37da909dda8e

## Results

| Strategy | Trades | Long / short | Wins | Win rate | Gross INR | Closed-trade drawdown INR |
|---|---:|---|---:|---:|---:|---:|
| Supertrend alone | 191 | 97 / 94 | 66 | 34.6% | 83,900 | 64,600 |
| Supertrend + MACD + VWAP | 143 | 74 / 69 | 54 | 37.8% | 108,100 | 46,200 |
| Earlier EMA/VWAP/MACD baseline | 69 | 36 / 33 | 14 | 20.3% | 101,500 | 33,100 |

Added confirmation improves this sample over Supertrend alone: fewer trades,
higher gross profit and lower closed-trade drawdown. Compared with the earlier
EMA/VWAP strategy, it has only INR 6,600 more gross profit, 74 extra trades and
INR 13,100 greater drawdown. Costs could change that ranking.
The EMA/VWAP strategy uses fixed 1.5 ATR stops, so the cross-strategy comparison
does not isolate the entry indicator. Drawdown excludes intratrade fluctuations.
Do not treat this retrospective sample as forward validation. No default changed.

## Files and reproduction

- supertrend.rs: band calculation, native ATR.
- supertrend_confirmation.rs: native MACD/VWAP calculations and directional confirmation.
- supertrend_actor.rs: entry-only confirmation, lifecycle, reversal exits and one-lot orders.
- supertrend_backtest.rs: native BacktestNode, Redis cache and native P&L reconciliation.
- supertrend_batch.rs: isolated daily processes, same-input original/confirmed comparison.
- supertrend_input.rs delegates validation/replay to the shared vwap_input.rs calendar.

Results:
backtest_results/supertrend_macd_vwap_2026-08-17_to_2026-09-15_2c404264-451e-4255-8a6a-20d0dbaad733/

    cargo run --locked -p kite-node -- native-supertrend-confirm-compare 2026-08-17 2026-09-15 backtest_results/supertrend_macd_vwap_2026-08-17_to_2026-09-15_2c404264-451e-4255-8a6a-20d0dbaad733/historical_input.json

Root comparison.json and README.md summarize results. original/ and confirmed/
contain daily native reports, indicators, signals, fills, trades and candles.
verification.json audits all 334 trades: exact input, directional confirmations,
causal next-open prices, Supertrend/EOD exits, fill quantities, flat daily state,
P&L arithmetic and drawdown. Native P&L is reconciled inside every session.

Real orders remain disabled. No broker requests are needed for this offline run.


Validation: 193 workspace tests passed, zero failed, one existing ignored fixture.
Clippy with warnings denied, formatting and diff checks passed.
The original Supertrend native integration test still verifies long/short orders,
flat shutdown and causal next-open execution. New confirmation tests cover strict
inequalities, readiness, negative-MACD bullish confirmation, positive-MACD bearish
confirmation, session VWAP reset and zero-volume readiness.

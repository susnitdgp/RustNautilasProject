# Supertrend + MACD + VWAP: five versus ten-minute candles

## Scope

17 August–15 September 2026 inclusive: 30 calendar days, 22 trading sessions.
Same CRUDEOIL26SEPFUT contract, one lot (100 barrels).
Both variants use Supertrend(7,2), MACD(12,26,9) and session VWAP.
No separate stop-loss, trailing stop, stop cooldown or profit target.
Exit only on completed-bar Supertrend reversal or session close.

Long: bullish Supertrend, close > session VWAP, MACD > signal.
Short reverses all three. No MACD zero-line test or EMA9/21 crossover.
When flat, enter once aligned; confirmation may arrive after the Supertrend flip.
Entry and reversal exit use the next candle open. Session liquidation uses last close.
All indicator periods remain unchanged in bars, so their time horizons are longer
on the ten-minute run. All indicators, including HLC3 VWAP, are recalculated on
the selected interval; ten-minute VWAP need not equal five-minute VWAP.

## Candle construction and audit

Ten-minute candles are formed from adjacent saved five-minute candles within a
session, aligned at 09:00 IST (17:00 on September 14).
Open = first open, high/low = pair max/min, close = second close,
volume = sum, OI = second value. No pairs cross sessions.
Incomplete, missing or misaligned pairs are rejected. There are 87 bars in a
full ten-minute session, 39 in the evening-only session: 1,866 evaluation bars.
435 earlier ten-minute bars provide warmup (870 five-minute bars).
Both intervals originate from the same unchanged historical input; no API fetch.

The input SHA-256 is:
    f05c41cdb8284150918a087159ae37ddc0957c01e855b41cdf9d37da909dda8e

## Results

| Metric | 5-minute | 10-minute |
|---|---:|---:|
| Evaluated bars | 3,732 | 1,866 |
| Trades | 143 | 89 |
| Long / short | 74 / 69 | 48 / 41 |
| Wins | 54 | 38 |
| Win rate | 37.8% | 42.7% |
| Gross P&L INR | 108,100 | 87,100 |
| Closed-trade drawdown INR | 46,200 | 49,900 |
| Worst trade INR | -14,900 | -23,200 |
| Profit factor | 1.377 | 1.362 |

Ten-minute candles reduce trades by 54 and increase win rate, but gross profit
falls by INR 21,000 and closed-trade drawdown rises by INR 3,700.
The largest individual loss is also higher. There is no overall improvement
in gross profit or drawdown in this sample. No default interval was changed.
Costs are excluded, and the smaller trade count could affect net comparisons.
Drawdown measures closed trades only. This retrospective comparison is not
independent forward validation.

## Reproduction and review

    cargo run --locked -p kite-node -- native-supertrend-interval-compare 2026-08-17 2026-09-15 backtest_results/supertrend_macd_vwap_2026-08-17_to_2026-09-15_2c404264-451e-4255-8a6a-20d0dbaad733/historical_input.json

Results:
backtest_results/supertrend_interval_comparison_2026-08-17_to_2026-09-15_19a53ea9-e08f-4868-815e-1d31516048fa/

comparison.json and README.md include aggregate/date-wise results.
5minute/ and 10minute/ retain daily indicators, signals, fills, trades, native
reports and input candles. Original and aggregated snapshots are at the root.

ten_minute.rs aggregates candles. vwap_input.rs provides interval-aware validation
and causal replay while the existing VWAP strategy remains five-minute-only.
supertrend_input.rs delegates to that replay; supertrend_backtest.rs uses the
matching native BarType and correct interval metadata.
supertrend_interval_batch.rs isolates daily native engines and builds the report.
The original Supertrend actor/confirmation logic is unchanged for this test.

verification.json audits every aggregation pair and all entries/exits, bar-close
timing, next-open prices, one-lot quantities, native P&L/flatness and drawdown.
The five-minute baseline trades reproduce exactly. Neither variant places stops.
Real orders remain disabled.


Workspace tests, Clippy with warnings denied, formatting and diff checks passed.
New tests verify OHLCV/OI pairing, reject missing/incomplete candles, check
ten-minute warmup coverage and bar-close timing, and reject a mismatched BarType.
Changes remain uncommitted.

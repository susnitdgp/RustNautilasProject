# Entry-filter comparison

Run from /home/ubuntu/RustNautilasProject:

    cargo run --locked -p kite-node -- native-vwap-compare 2026-09-15 backtest_results/vwap_ema_macd_5minute_7sessions_2026-09-15_bfd3c424-d640-4346-9cda-f4f63f40e1cc/historical_input.json

All three variants use one copied historical snapshot for the same seven sessions.
No fresh market-data requests or broker orders are made by this command.
The default native-vwap-backtest behavior remains baseline.

## Predeclared variants

| Variant | Entry rule |
|---|---|
| baseline | Original qualifying EMA 9/21 crossover, close vs session VWAP, MACD vs signal |
| trend | Baseline plus MACD strictly above zero for long/below zero for short, and both EMA 9 and EMA 21 rising/falling over the preceding bar |
| breakout | Original qualifying setup; wait for a later completed candle to close strictly above the setup high for long, or below its low for short |

Breakout is tested separately, not combined with the trend filter.
Confirmation is allowed only in the next three candles (15 minutes).
A wick beyond the level or a close exactly on the level is insufficient.
EMA ordering, close vs VWAP and MACD vs signal must remain aligned while waiting.
The pending setup is canceled on lost alignment, expiry or an IST session change.
Once confirmed, the setup is consumed. It does not automatically re-arm after a stop.
A new original qualifying crossover may arm a new setup.

## Controls held constant

- Five-minute CRUDEOIL26SEPFUT, one lot, 100-barrel multiplier.
- Native Nautilus VWAP, EMA 9/21, MACD 12/26/9 and ATR(14) Wilder.
- Entry occurs at the next candle open after the entry signal.
- Fixed 1.5 ATR stop from actual entry, outward-rounded to a rupee tick.
  ATR is from the completed entry-signal bar (the confirmation bar for breakout).
- Original opposite-setup exits remain active even if a new entry filter rejects
  the opposite entry. This avoids changing exit rules along with entry rules.
- All positions close daily. September 14 is the existing evening-only session.
- Same native stop matching, gap handling, Redis persistence and fill-P&L checks.
- Fees, spread and slippage excluded; closed-trade drawdown is not intratrade drawdown.
- No 15-minute higher-timeframe filter or stop widening was introduced.
- No parameter optimization is performed; the three-bar window was fixed before running.

## Review and reports

vwap_filters.rs contains the entry variants and setup state.
vwap_signal.rs records raw entries, one-bar EMA slopes and final filtered entries.
vwap_actor.rs preserves original opposite-setup exits independently of entry filters.
vwap_compare.rs runs and reports the comparison.
Existing backtest and batch runners accept explicit variants for child sessions.

Reports are saved under backtest_results/vwap_filter_comparison_DATE_UUID/.
The root has comparison.json, README.md and the exact copied historical_input.json.
Each baseline/trend/breakout directory has seven native daily runs and full traces.
Old reports are preserved.

This is an in-sample comparison on the same seven sessions already inspected.
Blocking the 15 September losing trade alone does not establish a better strategy.
Any chosen rule still needs evaluation on an independent period.
Real orders remain disabled.

## Results: 7–15 September 2026

| Variant | Trades | Wins | Gross P&L INR | Closed-trade drawdown INR |
|---|---:|---:|---:|---:|
| Original | 9 | 4 | 97,700 | 13,800 |
| Zero-line + EMA slopes | 2 | 1 | 12,500 | 5,100 |
| Breakout alone | 4 | 1 | 6,300 | 6,800 |

Both variants avoided the second 15 September loss. Trend made no trades that day;
breakout made one morning trade, losing INR 6,800. Neither variant improved gross
return across this sample. Lower closed-trade drawdown came with fewer trades.
The default remains the original strategy; no winning variant was promoted.

The original nine trades reproduced exactly. All six filtered entries passed an
independent audit of their conditions, completed-bar timing and next-open prices.
The saved input matched the previous run byte for byte (SHA-256):

    585c23f7a719a090caa6cf962685042e0ee44355d86c9b6ca5988e83c380d698

Result directory:

    backtest_results/vwap_filter_comparison_2026-09-15_5e44581b-84d0-4143-89e3-3fe383e59dda/

Verification: 190 workspace tests passed, zero failed, one existing ignored fixture.
Clippy with warnings denied, rustfmt and diff checks passed. Filter tests cover
both trade directions, strict zero-line/slope conditions, wick rejection, equality,
setup consumption, three-bar expiry, lost confirmation and session reset.

## Expanded comparison: 17 August–15 September 2026

Thirty calendar days, 22 trading sessions, 3,732 evaluated five-minute bars.
The same September contract and one-lot sizing are used throughout; no contract roll.
870 candles from 10–14 August warm up indicators and cannot generate trades.
These were fetched through the read-only Kite historical API; all original
17 August–15 September candles remain unchanged. No parameters were retuned.
The calendar follows https://www.mcxindia.com/market-operations/trading-surveillance/trading-holidays.

| Variant | Trades | Long / short | Wins | Win rate | Gross INR | Closed-trade drawdown INR |
|---|---:|---|---:|---:|---:|---:|
| Original | 69 | 36 / 33 | 14 | 20.3% | 101,500 | 33,100 |
| Zero-line + EMA slopes | 22 | 9 / 13 | 3 | 13.6% | -28,500 | 41,000 |
| Breakout alone | 37 | 18 / 19 | 7 | 18.9% | 27,200 | 34,500 |

Neither filter improves gross profit, win rate or closed-trade drawdown on this
expanded sample. The original default remains unchanged. Its gross profit is
concentrated in the previously inspected seven sessions: INR 97,700 there, versus
INR 3,800 over the earlier 15 sessions. This is not evidence of stable returns.
Earlier-period gross results were INR -41,000 for trend and INR 20,900 for breakout.
Trading costs remain excluded. Drawdown omits intratrade equity fluctuations.

Results and exact offline input:
backtest_results/vwap_filter_comparison_2026-08-17_to_2026-09-15_83cd24fa-b97f-4fec-a949-de59d3cb99d3/

Reproduce offline with:
    cargo run --locked -p kite-node -- native-vwap-compare-range 2026-08-17 2026-09-15 backtest_results/vwap_filter_comparison_2026-08-17_to_2026-09-15_83cd24fa-b97f-4fec-a949-de59d3cb99d3/historical_input.json

Verification: 191 workspace tests passed, zero failed, one existing ignored fixture.
Clippy with warnings denied, formatting and diff checks passed.
All overlapping seven-session trades reproduce exactly across every variant.
Input coverage, original candle equality, per-trade P&L, aggregate fills,
flat daily positions and closed-trade drawdown are audited in verification.json.
The range command fetches read-only warmup if the supplied input has fewer than
100 candles before the requested start. With the saved complete input it is offline.

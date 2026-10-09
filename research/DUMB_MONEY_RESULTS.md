# Dumb Money Concepts [theUltimator5] — CRUDEOIL exploratory P&L

Source: user-provided Pine v6 indicator. This is **not a published trading strategy**, and the author's original signals do not specify trade direction or exits. We implemented independent hypotheses using the user-supplied FOMO, panic flush, herd exhaustion, chase price and hopelessness signals. We did **not** assign trades to the 'boring price action' state or the DMI histogram; both need a separate entry rule. DDX was independently approximated; this is **not a verified bar-for-bar TradingView reproduction**.

Underlying: standard `CRUDEOIL26OCTFUT.MCX` October futures, historical Kite 3m and 5m bars with August 2026 used for warmup, September 1–30 and October 1–8 scored separately. Hypothetical fills at the next bar's open, ATR(14) stop = 1 ATR, target = 2R, max 12 bars, one position per signal family, session flat at entry cutoff. **2 points round-trip** and **0.5 point slippage each side** included. No Kite orders or broker-confirmed fills.

| Hypothesis | 3m Sep trades / net pts | 3m Oct trades / net pts | 5m Sep trades / net pts | 5m Oct trades / net pts |
|---|---:|---:|---:|---:|
| FOMO fade SHORT | 14 / +51.74 | 2 / -33.41 | 6 / -178.25 | 4 / -105.45 |
| FOMO breakout LONG | 14 / -24.12 | 2 / +51.82 | 6 / -181.22 | 4 / +50.62 |
| Panic flush LONG | 36 / -313.62 | 12 / +37.03 | 24 / -215.82 | 6 / -83.31 |
| Herd exhaustion SHORT | 11 / -115.90 | 1 / -17.78 | 5 / +30.78 | 5 / -55.61 |
| Chase fade SHORT | 12 / -94.66 | 5 / -60.40 | 10 / -83.49 | 5 / -124.55 |
| Hopelessness reversal LONG | 16 / -16.04 | 2 / -6.35 | 10 / +137.55 | 0 / 0.00 |

**Decision:** No evaluated hypothesis shows reliable positive expectancy on both periods; the positive September 5m hopelessness result has no October trades. October 3m FOMO breakout is just two trades. Independent signal-family simulations do not account for competing signals/portfolio overlap; their P&Ls are **not additive**. No claim of edge or deployability.

The code approximates Pine RSI/ADX initialization and daily session treatment; excludes the DMI normalized components, 'Indicator Crowd' signal (not present among actual `...Fires` triggers in supplied code), and 'Bored' directionless state. ATR stop, 2R target, holding time and FOMO direction tests are **our choices**, not author-defined. Since the original indicator has no complete execution system, this cannot establish the indicator's profitability. Next step, only if warranted, would be TradingView event parity checks and explicitly specified trade rules rather than optimistic optimization.

Run `python3 research/dumb_money_pnl.py` after exporting futures candles with `native-amd-export` (3 or 5). No connection to order execution.

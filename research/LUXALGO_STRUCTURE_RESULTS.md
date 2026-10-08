# LuxAlgo Market Structure with Inducements & Sweeps — exploratory CRUDEOIL P&L

**Source:** user-supplied LuxAlgo Pine Script v5, CC BY-NC-SA 4.0. Our independent research approximation tests the indicator's CHoCH, inducement (IDM), break of structure (BOS), and wick-rejection sweep events. Indicator code itself is not redistributed here. No claim of exact TradingView execution.

Data: October 2026 standard CRUDEOIL futures (`CRUDEOIL26OCTFUT.MCX`), September and October 1–8 sampled from Kite historical 3-minute/5-minute candles with earlier August warmup. Hypothetical entry: next candle open, fixed slippage 0.5 points per side, 2 points round-trip cost, one position at a time; BOS uses prior structural extreme stop, sweep uses rejection wick, target = 2R, maximum holding 12 bars, simulated day-flat at 23:15. Signals at swing *confirmation* times, not retrospectively at swing point. An additional exploratory exclusion rejects risk over 250 points. Trading rules are **new hypotheses**; original indicator contains no trading rules.

| Variant | September trades | September net points | September PF | October trades | October net points | October PF |
|---|---:|---:|---:|---:|---:|---:|
| 3m BOS after IDM | 25 | +37.5 | 1.084 | 7 | -180 | 0.305 |
| 3m sweep reversal | 167 | -669 | 0.644 | 51 | -42 | 0.921 |
| 5m BOS after IDM | 8 | +72 | 1.900 | 2 | -51 | 0.433 |
| 5m sweep reversal | 111 | -216.5 | 0.850 | 23 | -15 | 0.948 |

**Conclusion:** Neither sweep-reversal variant is profitable in either tested period. Both BOS variants earn points in September but lose in early October, without a robust edge. The 5m BOS has only 10 completed sample trades across both periods; results are inconclusive. These are modeled points, not rupees or broker-verified profit. Outcomes depend on stop, R target, timeout and intra-bar assumptions; no production changes.

Run `python3 research/luxalgo_structure_pnl.py`. This prototype should be independently checked against TradingView signals and actual executable quotes before using conclusions to trade.

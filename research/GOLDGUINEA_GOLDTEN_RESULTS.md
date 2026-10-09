# GOLDGUINEA versus GOLDTEN, October 2026 futures (research only)

Instrument master verified 2026-10-09: GOLDGUINEA26OCTFUT token 146254087, GOLDTEN26OCTFUT token 146254599; both expire 2026-10-30. GOLDGUINEA is 8 grams and GOLDTEN 10 grams. 5 GOLDGUINEA versus 4 GOLDTEN hedges 40 grams each way (contract quotation is Rs per contract). No order placement.

Kite historical 3-minute OHLCV retrieved using renewed access token (never displayed), August 3–October 8 2026: 13,760 candles each. Matched September candles 6,220, October 1–8 1,450. **Close spread** S = 5*GOLDGUINEA_close - 4*GOLDTEN_close. September min/median/max Rs 755 / 2,858 / 4,378. October min/median/max Rs 1,865 / 2,480 / 3,709. Median absolute 3m spread change September Rs 151, October Rs 131; this likely includes asynchronous prints, spread staleness, and independent microstructure noise. Thus OHLC candle closes are **not an executable arbitrage quote**.

A highly preliminary *close-spread proxy* uses a 120-bar trailing mean and standard deviation, entry when |Z| >= threshold, exit when spread meets mean or after 20 bars/session boundary, fills proxied by next candle closes. One spread position at a time. This is NOT a valid execution backtest. The table shows September and October results, each net of **hypothetical** total transaction friction in rupees per 40g round trip (5 + 4 contracts on entry AND exit). The ₹100/₹250/₹500 friction scenarios are sensitivity checks, **not estimates of actual transaction charges or bid/ask spread**.

| Threshold | Sept trades | Sept zero-friction | Sept ₹100 friction | Sept ₹250 friction | Oct trades | Oct zero-friction | Oct ₹100 friction | Oct ₹250 friction |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Z 1.5 | 374 | +31,482 | -5,918 | -62,018 | 95 | +6,868 | -2,632 | -16,882 |
| Z 2.0 | 198 | +21,233 | +1,433 | -28,267 | 54 | +3,797 | -1,603 | -9,703 |
| Z 2.5 | 95 | +13,102 | +3,602 | -10,648 | 26 | +2,568 | -32 | -3,932 |
| Z 3.0 | 29 | +2,931 | +31 | -4,319 | 5 | +401 | -99 | -849 |

**Decision:** No execution-verified arbitrage opportunity demonstrated. Close-price convergence is not tradable proof and the preliminary outcomes are fragile even under small hypothetical total costs. Need synchronized best bid and ask depths, contract multiplier/fees validation, stale quote filtering, and two-leg adverse selection/fill simulation before making any trade decision. Historical OHLC tests cannot prove riskless arbitrage.

Run `python3 research/goldguinea_goldten_spread.py` with historical JSON files `/tmp/goldguinea_oct_3m.json` and `/tmp/goldten_oct_3m.json`. These are local and untracked. No live trading configuration altered.

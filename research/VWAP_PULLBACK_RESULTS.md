# 1-minute VWAP pullback and EMA crossover — first-pass P&L

Instrument: standard CRUDEOIL October 2026 futures, Zerodha Kite 1-minute historical candles (August warmup, September and October 1–8 scored). The 1-minute strategy is proposed research, **not** a user-supplied exact Pine Script.

VWAP model: session-volume VWAP, EMA9 above/below EMA21, ADX14 above 22, 3-bar pullback contacting EMA9 in trend direction and holding VWAP, confirming candle closing beyond EMA9, skip 1.8 ATR oversized candles, next-candle-open entry. EMA baseline: simple EMA9/EMA21 crossover, next-candle-open entry.

Shared risk execution: stop at recent six-bar swing (clamped to 0.5–1 ATR), target 1R/1.5R/2R, maximum 10 bars, one position at a time, cutoff and day flat, stop-first ambiguous OHLC, stop after three consecutive losing trades in a session. All net results **include 2 point completed round-trip cost and 0.5 point adverse slippage each side**. Original suggested rules left pullback lookback, volume confirmation, daily loss limit and stop swing details underdetermined; this simulation fixes one reproducible interpretation. Volume is used for VWAP but not a hard >20-bar-average entry confirmation. No live order or signal-to-quote testing.

| Model | Target | Sept trades | Sept wins | Sept net points | Sept PF | Oct trades | Oct wins | Oct net points | Oct PF |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| VWAP pullback | 1R | 154 | 51 | -767.00 | 0.277 | 41 | 18 | -141.85 | 0.255 |
| VWAP pullback | 1.5R | 108 | 27 | -530.27 | 0.265 | 34 | 12 | -127.04 | 0.307 |
| VWAP pullback | 2R | 106 | 23 | -561.39 | 0.231 | 30 | 8 | -124.84 | 0.297 |
| EMA crossover | 1R | 132 | 38 | -651.46 | 0.235 | 81 | 48 | -109.03 | 0.715 |
| EMA crossover | 1.5R | 128 | 35 | -604.27 | 0.291 | 65 | 32 | -103.15 | 0.723 |
| EMA crossover | 2R | 113 | 28 | -477.85 | 0.369 | 44 | 20 | -67.53 | 0.728 |

**Verdict:** No positive month or target, and VWAP pullback underperformed EMA baseline. Cannot justify live deployment. This is a one-interpretation approximation; fill and intrabar data limitations apply, as do small October samples. No optimization to September or October undertaken.

Reproduce: `python3 research/vwap_pullback_1m.py` using /tmp/amd_crudeoil_1m.json on research host. Script uses no broker API or order execution.

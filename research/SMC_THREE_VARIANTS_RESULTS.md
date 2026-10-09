# LuxAlgo SMC variants A/B/C — CRUDEOIL research

Exploratory independent causal structure approximation, NOT faithful bar-for-bar replication of LuxAlgo Smart Money Concepts Pine v5.

October 2026 standard CRUDEOIL futures. Kite 3m and 5m candles, August warmup, September 2026 and October 1–8 scored separately. Next bar open entries, 0.5-point adverse slippage on both entry and exit, 2-point round trip, stop-first OHLC, max 30-minute hold, session controls. 15m bias approximated by completed 15m close versus EMA20. Confirmed 5-left/5-right pivots approximate internal structure; CHoCH then BOS followed by proxy last opposite candle for internal OB, optional aligned simple 3-bar FVG. No unconfirmed pivot is used as a signal. Research choices, not explicitly LuxAlgo's full implementation.

Caution: Variant A's swing-structure stop generally exceeds the added 1.5 ATR risk ceiling, resulting in only one executed trade in each timeframe across the scored periods; **A is NOT adequately tested and must not be interpreted as profitable**. B/C use last opposite candle as proxy OB, not LuxAlgo parsed-high/low OB selection, and FVG uses a simplified 3-candle rule rather than the original optional HTF gap engine. These are hypothesis studies, not script-equivalent backtests.

| TF | Variant | Target | Sept trades | Sept net pts | Sept PF | Oct trades | Oct net pts | Oct PF |
|---|---|---|---:|---:|---:|---:|---:|---:|
| 3m | A structure | 1.5R | 1 | +22.00 | n/a | 0 | 0 | n/a |
| 3m | A structure | 2R | 1 | +22.00 | n/a | 0 | 0 | n/a |
| 3m | B structure + OB | 1.5R | 37 | -107.39 | .738 | 10 | -20.78 | .813 |
| 3m | B structure + OB | 2R | 36 | -129.86 | .690 | 10 | +3.12 | 1.028 |
| 3m | C structure + OB + FVG | 1.5R | 32 | -74.26 | .776 | 9 | -4.45 | .953 |
| 3m | C structure + OB + FVG | 2R | 31 | -108.04 | .684 | 9 | +19.46 | 1.206 |
| 5m | A structure | 1.5R | 0 | 0 | n/a | 1 | +20.00 | n/a |
| 5m | A structure | 2R | 0 | 0 | n/a | 1 | +20.00 | n/a |
| 5m | B structure + OB | 1.5R | 27 | -225.47 | .501 | 12 | -4.05 | .974 |
| 5m | B structure + OB | 2R | 27 | -247.41 | .455 | 12 | +29.99 | 1.194 |
| 5m | C structure + OB + FVG | 1.5R | 23 | -187.54 | .510 | 11 | +22.28 | 1.174 |
| 5m | C structure + OB + FVG | 2R | 23 | -220.75 | .426 | 11 | +56.33 | 1.441 |

**Verdict:** B and C are negative in September for all tested combinations. Positive October windows are very small. No robust cross-month profitable edge demonstrated. Variant A has practically no filled trades because of a restrictive risk ceiling. Not deployable.

Run `python3 -B research/smc_three_variants.py` using local `/tmp/amd_crudeoil_3m.json` and `/tmp/amd_crudeoil_5m.json`. No orders or production changes.

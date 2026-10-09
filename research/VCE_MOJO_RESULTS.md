# VCE-Mojo v1.6 — CRUDEOIL 3m/5m first-pass P&L

Source: user-supplied VCE-Mojo Volatility Coil Edge with AlgoMojo exit events, Pine v6.

An independently written research approximation models Balanced default parameters: ATR4/ATR20 contraction or median candle-range compression or catalyst, 4–14 compression bars, 2 tolerance violations, session-anchored 50-bar extremes, 15-bar session warm-up, at-high short versus at-low long watch, 10-bar watch expiration, confirmed edge break, no new entry at/after 23:15 IST, EOD exit and three-bar post-outcome gap. Stop is coil opposite edge plus/minus 0.2 ATR clamped to 0.3–2 ATR; TP1=1R, TP2=1.5R, TP3=2R. Exits are full position at selected target or SL, SL-first same candle. The strategy's own indicator uses signal-close reference entries. This research instead uses next-candle-open with 0.5-point adverse slippage per entry/exit plus 2 points round-trip cost.

October 2026 standard CRUDEOIL future, synced 3m/5m historical Kite candles, August 2026 warmup, September and October 1–8 scored separately.

| TF / exit | Sept trades | Sept wins | Sept net pts | Sept PF | Oct trades | Oct wins | Oct net pts | Oct PF |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 3m TP1 default | 127 | 54 | -633.41 | 0.706 | 25 | 10 | -248.68 | 0.520 |
| 3m TP2 | 118 | 40 | -692.98 | 0.704 | 21 | 8 | -78.00 | 0.805 |
| 3m TP3 | 110 | 32 | -749.64 | 0.683 | 20 | 8 | +59.33 | 1.160 |
| 5m TP1 default | 68 | 24 | -885.09 | 0.503 | 13 | 8 | +96.52 | 1.395 |
| 5m TP2 | 67 | 22 | -601.94 | 0.669 | 13 | 8 | +261.54 | 2.072 |
| 5m TP3 | 66 | 22 | -197.34 | 0.888 | 12 | 5 | +30.10 | 1.087 |

**Verdict:** No cross-month profitable candidate. Original user-provided Pine has no independent TradingView validation; this Python port was not reconciled bar-for-bar for ATR/pivots/session resetting, ambiguous fills, TradingView bars/timezone or real execution. Oct results small sample. The 5m TP2 alternative was tested after seeing the script's configurable exits; avoid cherry-picking. No production changes or broker orders.

Reproduce: python3 research/vce_mojo_pnl.py (reads /tmp/amd_crudeoil_3m.json and /tmp/amd_crudeoil_5m.json on research host).

## 1-minute experiment

Kite 1m CRUDEOIL October future, September 2026 and October 1-8. Next-bar-open entries, 2 points round trip and 0.5-point slippage each side. Independent approximation, not exact Pine.

| Target | Sept trades | Sept net pts | Sept PF | Oct trades | Oct net pts | Oct PF |
|---|---:|---:|---:|---:|---:|---:|
| TP1 default 1R | 466 | -2215.47 | 0.538 | 108 | -170.70 | 0.836 |
| TP2 1.5R | 439 | -2199.16 | 0.580 | 92 | -327.84 | 0.720 |
| TP3 2R | 405 | -1988.90 | 0.618 | 83 | -176.38 | 0.841 |

No profitable tested month or target. 1m data locally cached, not committed. Exporter now supports 1m, 3m and 5m.

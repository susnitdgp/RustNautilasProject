# Synapse Trail Pro v1.4 — CRUDEOIL first-pass P&L

Independent approximation of core default signals (EMA21, ATR13 ×1.618 ratchet) and Balanced risk (1.5 ATR stop, 1R/2R/3R targets in thirds, break-even after TP1).

October 2026 standard futures, 3m/5m Kite historical bars, August warmup, September and October 1–8 separately. Next-candle open fills, 2 points round trip plus 0.5 point adverse slippage each side. Stop-first ambiguities, daily flat rule. These latter rules and actual execution differ from Pine's signal-close hypothetical entry and default zero fees.

| Timeframe | September trades | September wins | September net pts | Sept PF | October trades | October wins | October net pts | Oct PF |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 3m | 151 | 76 | -179.17 | 0.902 | 29 | 11 | -123.17 | 0.759 |
| 5m | 95 | 39 | -533.33 | 0.729 | 20 | 14 | +207.33 | 1.888 |

**No demonstrated cross-month edge.** Approximate, not independently reconciled bar-for-bar with TradingView. Missing optional score/HTF display; possible differences in seed ATR, intrabar exit ordering, target handling, partial-exit slippage and session modeling. No orders placed. Run: python3 research/synapse_trail_pro_pnl.py (requires cached historical JSON files).

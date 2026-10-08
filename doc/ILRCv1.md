# ILRC v1 — selected 3-minute research/paper profile

Selected instrument: `CRUDEOIL26OCTFUT.MCX`  
Timeframe: **3 minutes**  
Contracts: **1**  
Live orders: **disabled**

ILRC v1 is an objective liquidity-event strategy:

1. Sweep previous-day or confirmed 20-bar swing liquidity.
2. Require displacement within three bars.
3. Require internal structure break and VWAP-side confirmation.
4. Enter only on a retracement within five bars.
5. Place structural stop beyond the sweep with a 0.15 ATR buffer.
6. Reject stops outside 0.5–1.5 ATR.
7. Require at least 1.5R to the nearest opposing external liquidity.
8. Force session flattening; no live broker-order path is authorized.

Current selected parameters live in `config/production-ilrc.json`.

Research evidence so far is promising but statistically small. This profile is therefore selected for paper/research validation only. SMBC remains available as a separate fallback implementation.


## Break-even protection

The selected production profile uses a conservative single-contract break-even rule:

- After an open trade reaches **+1.0R** in favorable excursion, the stop moves to the entry price.
- The move applies from the **next completed bar**; the trigger bar is still evaluated against the original stop.
- The original opposing-liquidity target remains unchanged.
- No partial profit-taking or trailing stop is used.

This was adopted after cross-contract CRUDE research because it reduced failed follow-through losses without increasing trade frequency. Liquidity-pool strength, Mirage-style sweep scoring, displacement close-quality, and SATS efficiency-ratio filters remain research-only because they did not improve results consistently across the tested expiries.


## Combined production-shadow profile

The selected shadow profile evaluates two setup families under one-position-at-a-time arbitration:

1. Setup A - ILRC reversal: liquidity sweep, displacement, retracement, opposing-liquidity target, break-even after +1R.
2. Setup B - ILRC continuation: 20-bar external structure break, ILRC-strength displacement, VWAP-aligned retracement entry, 3R target, break-even after +1R.

If both overlap, the active trade blocks later candidates. On identical entry timestamps Setup A has priority. Shadow history labels exits A_TP/A_SL/A_BE/A_EOD or B_TP/B_SL/B_BE/B_EOD.

This remains a production-shadow profile only. Live order submission is disabled.

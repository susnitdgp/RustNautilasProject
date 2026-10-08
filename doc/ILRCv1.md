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

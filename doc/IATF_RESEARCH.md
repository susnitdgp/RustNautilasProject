# IATF Strategy C — CRUDEOIL MINI research only

This module is **not** a Nautilus strategy actor or an order-sending path. It implements a deterministic, tick-driven prototype for Kaufman efficiency regime classification, top-of-book *resting* quantity imbalance, and breakout candidate generation. The sample instrument is an explicitly **unverified placeholder**; no actual instrument token, contract expiry or lot size is asserted.

Run: `./target/release/kite-node native-iatf-research-validate config/iatf-crudeoilmini-research.json`.

The config is disabled; `live_orders_enabled: true` is rejected. No subscriptions, Redis operations, credentials, broker calls or trading are performed by the new CLI command.

The prototype requires strictly newer timestamps and fresh full-depth-derived quotes with bid/ask prices and quantities. Wrong instrument tokens, quote staleness, malformed depth and excess spread fail closed. Top-of-book imbalance is not executed trade delta. All thresholds are hypotheses, **not optimized**.

**Before testing signals against real data**: verify exact CRUDEOILM October 2026 expiry, exchange symbol, instrument token and size using the official broker instrument master; collect time-stamped depth snapshots with packet generation and gap flags; implement an offline record/replay harness and include realistic spread, slippage and fees. Do not claim profitability or leading prediction from this skeleton.

**Before any live rollout**: integrate with independent strategy engine orchestration, warmup, reconnection recovery, expiry/session validation, portfolio margin/risk, verified stop placement and broker reconciliation. ILRC and the existing live runner are untouched.

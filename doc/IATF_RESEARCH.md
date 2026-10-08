# IATF Strategy C — CRUDEOIL MINI research only

This module is **not** a Nautilus strategy actor or an order-sending path. It implements a deterministic, tick-driven prototype for Kaufman efficiency regime classification, top-of-book *resting* quantity imbalance, and breakout candidate generation. The sample instrument is an explicitly **unverified placeholder**; no actual instrument token, contract expiry or lot size is asserted.

Run: `./target/release/kite-node native-iatf-research-validate config/iatf-crudeoilmini-research.json`.

The config is disabled; `live_orders_enabled: true` is rejected. No subscriptions, Redis operations, credentials, broker calls or trading are performed by the new CLI command.

The prototype requires strictly newer timestamps and fresh full-depth-derived quotes with bid/ask prices and quantities. Wrong instrument tokens, quote staleness, malformed depth and excess spread fail closed. Top-of-book imbalance is not executed trade delta. All thresholds are hypotheses, **not optimized**.

**Before testing signals against real data**: verify exact CRUDEOILM October 2026 expiry, exchange symbol, instrument token and size using the official broker instrument master; collect time-stamped depth snapshots with packet generation and gap flags; implement an offline record/replay harness and include realistic spread, slippage and fees. Do not claim profitability or leading prediction from this skeleton.

**Before any live rollout**: integrate with independent strategy engine orchestration, warmup, reconnection recovery, expiry/session validation, portfolio margin/risk, verified stop placement and broker reconciliation. ILRC and the existing live runner are untouched.

## Offline JSONL replay (prototype)

Run `./target/release/kite-node native-iatf-replay config/iatf-replay.example.json PATH_TO_RECORDED_QUOTES.jsonl`.

Each record must contain: `generation`, `instrument_token`, `exchange_ts_ms`, `received_ts_ms`, `bid`, `ask`, `bid_qty`, `ask_qty`, `last`. Quotes must be ordered and must match the configured token. The sample replay token `42` is **synthetic**. Invalid quotes, spread, missing data, token changes or generation changes fail closed; no gap bridging or reconnect recovery is assumed. The current engine requires strictly increasing exchange timestamps, so same-millisecond or same-second quotes cannot yet be faithfully replayed if they map to the same timestamp. A sequence-number or receive-timestamp tie-breaker is a future requirement for production-grade collection.

Candidates are paper-filled at the **following** observation's opposite quote with adverse slippage. One paper position at a time, fixed hypothetical stop and target (plus a maximum quote-count duration), with per-side commissions. At EOF, open or pending exposure is marked **unresolved**, never silently liquidated. P&L and drawdown are realized-only and exclude unresolved exposure. Stop/target trigger uses observed last price, then observed executable quote and adverse slippage; prices may gap through stop thresholds. The replay is deliberately simplistic: it does **not** guarantee order fills, actual position sizing, exchange fees, taxes, latency or market impact. No trading commands exist in the replay.

JSONL recording from a live WebSocket is **not yet connected**. Only replay from already obtained, explicitly sourced depth records is implemented. Do not claim historical order-flow performance from OHLCV.

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

## September 2026 historical candle baseline

`./target/release/kite-node native-iatf-september-baseline 145894663` uses the existing read-only Kite historical 3-minute client. The Kite instrument master identifies the October futures as `CRUDEOILM26OCTFUT`, token `145894663`, expiry `2026-10-19`. The backtest command does not submit broker orders. Regime efficiency is evaluated separately within each trading day using previous completed closes, with candidate breakouts compared against previous eight closes. It does not reconstruct historical depth or calculate IATF trade P&L. Results are descriptive and cannot establish an order-flow edge.

## Manual Kite depth recording

To record real snapshots (no order execution), explicitly run:

`./target/release/kite-node native-iatf-record config/iatf-recorder.example.json`

The recorder validates CRUDEOILM26OCTFUT token 145894663 / expiry 2026-10-19 against Kite's public instrument master, reads the existing Redis credentials, and opens a **read-only**, bounded Kite WebSocket subscription. Output JSONL is created with `create_new` (never overwritten) under `data/iatf-recordings/` and is intentionally Git-ignored. Any gap or reconnect renders the capture unusable. Do not use the output for replay if capture reports failure. Historical September 2026 book snapshots cannot be recovered retroactively through Kite's candle endpoint. Its full-feed order updates are not used for trade execution.

## September 2026 1-minute vs completed 3-minute regime study

Run `./target/release/kite-node native-iatf-multitimeframe-september 145894663`. It fetches all September 2026 1-minute and 3-minute CRUDEOILM October futures bars with the existing authenticated read-only Kite historical client. It compares a prior-eight-close 1-minute breakout against the same event gated by a 10-bar Kaufman efficiency ratio calculated exclusively from completed 3-minute bars (threshold 0.45). It resets history at the session boundary and counts missing minutes. For eligible events, it measures the price difference between the next 1-minute open and the 10th subsequent 1-minute close, without overlapping-day lookahead.

Observed September baseline: 18,660 one-minute bars, 6,220 three-minute bars, 22 trading dates, 0 within-session minute gaps. Unfiltered: 5,998 candidate occurrences, 5,926 forward events, average directional movement -0.818 points. 3-minute trend-filtered: 1,547 candidate occurrences, 1,528 forward events, +0.467 points. **These are heavily overlapping event occurrences, not trades or net returns.** This is an exploratory result on one month, not out-of-sample evidence. Depth/OFI confirmation, executable stops, charges, and trade-level risk are not modeled; the result is not deployable.

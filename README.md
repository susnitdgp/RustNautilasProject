# Rust NautilusTrader / Zerodha Kite Research Workspace

Reusable Rust Kite integration, NautilusTrader components and read-only multi-asset portfolio validation. The discarded ILRC strategy, its runners and its research files were removed from the active tree. Older Git history remains available. No strategy is approved for live trading.

## SATS v1.13.1 (Rust port)

`crates/sats` is a bar-for-bar port of the Self-Aware Trend System v1.13.1 (WillyAlgoTrader): presets, adaptive
asymmetric SuperTrend with Trend Quality Index and character-flip, score, dynamic TPs, the script's single model
position (SL-first, thirds at TP1/TP2, TP3, flip and timeout exits, tick rounding, fees, slippage) and the
experimental self-learning calibration. All inputs live in the slot's own JSON (`config/sats-crudeoilm.json`).

Backtest a slot (read-only, Kite historical candles, no orders):

    ./target/release/kite-node native-sats-backtest config/portfolio-development.example.json crudeoilm-sats-202610 2026-09-01 2026-10-08

Writes `backtest_results/sats/<slot>/<run>/summary.json` and `trades.csv`: the `model_*` columns reproduce the
script's own R accounting (TradingView trader card); the `exec_*` columns apply the slot's `execution` settings
(lots, thirds or single exit, next-open market fills, slippage, round-trip cost).

### Running SATS on the market (CRUDEOILM slot)

Settings: `config/sats-crudeoilm.json` (5m, Dynamic TP, 1 lot, full exit at TP1, square-off 23:15 IST, MIS),
calendar `config/mcx-session-calendar.json` (reviewed through the contract expiry 2026-10-19).

* Paper (live Kite data, Kite mock execution, no broker orders): `deploy/run-sats-paper.sh`
* Live (REAL Zerodha orders):
  1. `cargo build --release --features live-orders`
  2. `config/portfolio-production.json` has the slot `enabled` and `live_orders_enabled`
  3. local, git-ignored `config/kite-production.json` has `live_orders_enabled: true` and your Kite user ID
  4. `deploy/run-sats-live.sh` runs the preflight, then asks you to type `LIVE`

Each run covers one trading day: start it after 09:00 IST; it warms SATS on broker-finalised history, trades,
flattens on the 23:15 bar and stops. A WebSocket gap, reconnect or invalid packet fails closed (flatten + stop),
any order rejection or position mismatch halts new orders and flattens. The bar in progress when the feed
connects is skipped (its ticks cannot be proven complete), so SATS sees a one-bar gap at start-up.
Logs (JSON lines) go to `logs/`. After the contract expiry, roll the slot (instrument, token, rollover block).

### State outside the process (named from the portfolio manifest)

A run keeps its order records, strategy state and Nautilus cache in memory only; there is no
order journal and no crash recovery (kite-node 2.21.0). Kite is the source of truth: every
live start requires a flat account with no open orders.

| Name | Purpose |
|---|---|
| file `~/.local/state/kite-node/locks/<prefix>-<slot>-<kite user>.lock` | one process per slot and account (OS lock, freed when the process ends; `KITE_LOCK_DIR` overrides the directory) |
| Redis `<prefix>:v1:{account-<kite user>}:order-budget` | order-rate budget, shared by all slots trading that Kite account |

Production uses `kite-prod` (`config/portfolio-production.json`), paper uses `kite-dev` and the pseudo-account
`PAPER`, so the two never share a lock or budget. `native-portfolio-validate` prints each slot's names.


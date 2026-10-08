# ILRC Combined — Rust NautilusTrader / Zerodha Kite

**Instrument:** MCX `CRUDEOIL26OCTFUT.MCX` · **interval:** 3-minute candles · **contracts:** 1 · **configuration:** `config/production-ilrc.json` · **expiry:** 19 October 2026. A new contract requires independently verified symbol, token, expiry, lot size, and session calendar.

## Strategy

ILRC Combined arbitrates two setups under **one-position-at-a-time** semantics. An active Setup A blocks B and vice versa; for simultaneous timestamps Setup A has priority.

| | Setup A — liquidity reversal | Setup B — continuation |
|---|---|---|
| Entry idea | Sweep prior-day / 20-bar liquidity, displacement and VWAP/internal structure confirmation, retracement within 5 bars | Break 20-bar external structure with displacement and VWAP direction confirmation, then retracement |
| Stop | Beyond swept liquidity plus ATR buffer | Structural anchor plus ATR buffer |
| Target | Opposing liquidity, at least 1.5R | 3R |
| Protection | Move stop to break-even once +1R reached; modification effective after confirmation | Move stop to break-even at +1R; 3R target unchanged |

The historical evaluator uses bar-internal price touches; executable fills after the candle closes may differ considerably. On 7 October 2026, the original completed-trade backtest showed five trades/+68.93 gross points; a next-open mock showed five fills/+5.80 gross points at zero slippage and about +0.80 with 0.5-point adverse slippage each side, before fees. Neither simulation establishes real-trading profitability.

## Manual production-shadow runner (read-only)

```bash
cd /home/ubuntu/RustNautilasProject
cargo build --release --locked -p kite-node
bash deploy/verify-ilrc-production.sh
bash deploy/run-ilrc-production-shadow.sh
```

To stop, use Ctrl+C. This shadow path **never creates an execution client or places real orders**. Do not install or start a service for the current manual workflow.

## Nautilus native broker mock (no real orders)

```bash
cargo run --locked -p kite-node -- native-ilrc-nautilus-mock config/production-ilrc.json 30
```

Historical regression fixture (requires `data/ilrc-test/real-2026-10-07.json` on the machine):

```bash
cargo run --locked -p kite-node -- native-ilrc-nautilus-fixture config/production-ilrc.json data/ilrc-test/real-2026-10-07.json 35
```

These commands create a Nautilus `LiveNode` with real read-only data, Redis-backed native mock execution, and the ILRC strategy actor. The mock accounts are unique per run. Any unresolved mock exposure requires manual review; existing Redis ownership records are not silently cleared.

## Quality checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
bash deploy/verify-ilrc-production.sh
bash deploy/verify-ilrc-live-integration.sh
```

## Execution status

The repository includes Kite HTTP order transport, Nautilus native order translation, Redis ownership and a broker-observation dispatcher, plus a new ILRC `LiveNode` actor tested against native mock execution. **No ILRC production order-sending command is active**, and `live_orders_enabled` must remain `false` in both strategy and broker settings. A mock acceptance or broker-modeled protective stop must not be mistaken for a validated live Zerodha stop.

See [ILRC strategy specification](doc/ILRCv1.md) and [Nautilus engineering architecture](doc/ILRC_ENGINEERING.md).

## Manual real Kite execution (separate live profile)

The live-capable binary is compiled using `cargo build --release --locked -p kite-node --features kite-adapter/live-orders`. Live execution uses the distinct `native-ilrc-nautilus-live` CLI and the **production** Kite execution client, never the native mock factory. The read-only preflight is:

```bash
./target/release/kite-node native-ilrc-live-preflight config/production-ilrc-live.json config/kite-ilrc-live.json
```

The manual order-sending command (run only when explicitly accepting real-money order risk, during a verified MCX trading session) is:

```bash
./target/release/kite-node native-ilrc-nautilus-live config/production-ilrc-live.json config/kite-ilrc-live.json 3600
```

Both live profiles are private local files ignored by Git. They must enable live orders, agree on the CRUDEOIL instrument token, identify the exact broker user, and use MIS / automatic market protection. These are **real-market orders**, not simulated orders. A clean startup requires a broker-flat account and no pending orders. Stop-loss placement follows the entry fill, so execution risk exists between those operations. The historical mock tests and read-only preflight are not proof of successful real-money order execution, protective stop acceptance or crash recovery. Review the broker orderbook/positions directly, ensure risk capital and margins are appropriate, and use Ctrl+C to request controlled shutdown. If there is unprotected exposure or an uncertain mutation, manual broker intervention may be necessary. The existing shadow command remains unchanged.

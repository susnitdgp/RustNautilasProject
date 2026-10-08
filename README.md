# Rust Nautilus + Zerodha Kite — ILRC Combined

Active strategy: ILRC Combined 3-minute CRUDEOIL **production shadow**. Broker orders are disabled and no execution client is loaded by the shadow runner.

Setup A: liquidity-sweep reversal, opposing-liquidity target, break-even at +1R. Setup B: 20-bar structure break, displacement and VWAP-aligned retracement, 3R target, break-even at +1R. Only one position at a time; Setup A wins identical entry timestamps.

## Manual foreground operation

```bash
cargo build --release --locked -p kite-node
bash deploy/verify-ilrc-production.sh
bash deploy/run-ilrc-production-shadow.sh
```

Stop the manual run with Ctrl+C. No systemd installation is required or recommended for the current manual workflow.

## Validation

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
bash deploy/verify-ilrc-live-integration.sh
cargo run --locked -p kite-node -- native-ilrc-mock-execution
```

See [ILRC strategy and replay documentation](doc/ILRCv1.md) for historical replay, causal entry-event audit, mock slippage/restart testing, and limitations. Live order routing and broker recovery are **not** ready. Keep strategy and broker `live_orders_enabled=false`.

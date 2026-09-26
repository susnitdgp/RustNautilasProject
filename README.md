# Rust Nautilus + Zerodha Kite

Native Rust/NautilusTrader project for the **MCX Crude Oil Trend Ribbon v2.10** strategy using Zerodha Kite market data and execution.

## Current production candidate

- Strategy: Trend Ribbon [BOSWaves] v2.10
- Instrument: `CRUDEOIL26OCTFUT.MCX`
- Timeframe: 5 minutes
- Intrabar engine: Kite full ticks / LTP
- FAST reversal: 2-second hold + ATR strength
- Pre-close reversal: final 3 seconds
- WaveTrend: dynamic ATR-based arm + peak/trough pullback exit
- Session: 09:00–23:15 Asia/Kolkata with daily reset
- Order model: reducing exit first, then opposite entry
- Persistence: Redis
- **Real orders remain disabled in the committed candidate config**

The authoritative strategy selection is `config/production-trend-ribbon.json`.
Private/local broker settings live in `config/kite-production.json`.

## Important files

- `apps/kite-node/src/native_node/trend_ribbon.rs` — confirmed-bar Trend Ribbon
- `apps/kite-node/src/native_node/trend_ribbon_realtime.rs` — FAST, pre-close and live WaveTrend logic
- `apps/kite-node/src/native_node/trend_ribbon_backtest.rs` — historical confirmed-bar v2.10 replay
- `apps/kite-node/src/native_node/trend_ribbon_replay.rs` — recorded Kite full-tick replay
- `apps/kite-node/src/native_node/trend_ribbon_recorder.rs` — read-only full-tick recorder
- `doc/TrendRibbonBOSWaves.md` — strategy/runtime details
- `deploy/verify-trend-ribbon-v210.sh` — complete offline verification
- `deploy/record-trend-ribbon-ticks.sh` — read-only market-data capture
- `deploy/run-trend-ribbon-live.sh` — live launcher

## Build and verify

```bash
cargo build --locked -p kite-node
./deploy/verify-trend-ribbon-v210.sh
```

The verifier checks formatting, strict Clippy, the full test suite, historical
Trend Ribbon/WaveTrend replay, saved full-tick replay, and Nautilus sandbox
FAST-reversal execution.

## Record a real tick session

During an open market session:

```bash
./deploy/record-trend-ribbon-ticks.sh 3600
```

The recorder creates a native full-tick catalog for later TradingView parity
comparison. It creates **no strategy and no execution client**.

Replay a saved capture with:

```bash
./target/debug/kite-node native-trend-ribbon-replay \
  config/production-trend-ribbon.json \
  data/native-catalog/<RUN_UUID>
```

## Production

Build the live-capable binary only when needed:

```bash
cargo build --locked --release -p kite-node --features kite-adapter/live-orders
```

Starting `deploy/run-trend-ribbon-live.sh` can submit real orders when all
compile-time and runtime safety gates are deliberately enabled. Before any live
session, verify the exact Kite position/open orders and the selected contract.

## Recovery

Redis is the source of truth for order/application state. A failed or uncertain
run must be reviewed before restart; never clear Redis broadly to bypass an
ownership or reconciliation gate.

Useful read-only commands:

```bash
./target/release/kite-node native-kite-review RUN_UUID
./target/release/kite-node native-kite-status EXPECTED_USER_ID
```

## Repository layout

```text
apps/kite-node/       application and strategy runtime
crates/               Kite adapters, execution, persistence helpers
config/               active operational configuration only
deploy/               current Trend Ribbon operational scripts
doc/                  current Trend Ribbon documentation
tests/fixtures/       regression-only legacy fixtures
vendor/               pinned NautilusTrader 0.63.0 patches
```

Generated build output, backtest reports, logs and runtime captures are ignored
by Git and should not be committed.

NautilusTrader is pinned to 0.63.0 with local compatibility patches documented
in `vendor/README.md`.

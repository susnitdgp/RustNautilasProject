# Rust Nautilus + Zerodha Kite

Native Rust/NautilusTrader project for the **MCX Crude Oil Trend Ribbon v2.23 Exit-First** strategy using Zerodha Kite market data and execution.

## Current production candidate

- Strategy: Trend Ribbon [BOSWaves] - FAST + SQZ Exit + Reentry Exit-First v2.23
- Instrument: `CRUDEOIL26OCTFUT.MCX`
- Timeframe: 5 minutes
- Intrabar engine: Kite full ticks / LTP
- FAST reversal: 2-second hold + ATR strength
- Pre-close reversal: final 3 seconds
- Squeeze Momentum: transition-gated exit + one same-trend continuation re-entry
- Session: 09:00–23:15 Asia/Kolkata with daily reset
- Order model: reducing exit first, then opposite entry
- Persistence: Redis
- **Real orders remain disabled in the committed candidate config**

The authoritative strategy selection is `config/production-trend-ribbon.json`.
Private/local broker settings live in `config/kite-production.json`.

## Important files

- `apps/kite-node/src/native_node/trend_ribbon.rs` — confirmed-bar Trend Ribbon
- `apps/kite-node/src/native_node/trend_ribbon_realtime.rs` — FAST, pre-close and live Squeeze Momentum logic
- `apps/kite-node/src/native_node/trend_ribbon_backtest.rs` — historical confirmed-bar v2.23 replay
- `apps/kite-node/src/native_node/trend_ribbon_replay.rs` — recorded Kite full-tick replay
- `apps/kite-node/src/native_node/trend_ribbon_recorder.rs` — read-only full-tick recorder
- `doc/TrendRibbonBOSWaves.md` — strategy/runtime details
- `deploy/verify-trend-ribbon-v223.sh` — complete offline verification
- `deploy/record-trend-ribbon-ticks.sh` — read-only market-data capture
- `deploy/run-trend-ribbon-live.sh` — live launcher

## Build and verify

```bash
cargo build --locked -p kite-node
./deploy/verify-trend-ribbon-v223.sh
```

The verifier checks formatting, strict Clippy, the full test suite, historical
Trend Ribbon/Squeeze replay, saved full-tick replay, and Nautilus sandbox
FAST-reversal execution.

## Kite authentication (Rust only)

The project includes a Rust-only Kite login/token utility.

Print the login URL without changing Redis:

```bash
./target/debug/kite-node native-kite-auth-login-url
```

For the full login flow, provide the permanent API secret through
`KITE_API_SECRET` (preferred for one-off use) or Redis key
`susanta:kite_api_secret`, then run:

```bash
./target/debug/kite-node native-kite-auth
```

The command prints the Kite login URL, waits for the returned `request_token`
or full redirect URL, exchanges it with Kite, and durably replaces only
`susanta:kite_access_token`. The API secret and access token are never printed.

## Terminal dashboard

The Rust-only Ratatui dashboard now combines the trade ledger with a right-side strategy monitor. The monitor shows current Ribbon/ALMA bands, ATR/deviation/slope, FAST/pre-close state, Squeeze value/state/transition/exit/re-entry state, plus current-trade MFE, MAE, giveback and retained-profit percentage.

Historical replay remains fully read-only and creates no strategy or execution client:

```bash
./target/debug/kite-node native-trend-ribbon-dashboard-history \
  config/production-trend-ribbon.json 2026-09-22
```

Controls: `q`/Esc quits the replay dashboard, Space pauses/resumes, Left/Right steps one 5-minute bar, Home/End jumps to the first/last bar, and `+`/`-` changes replay speed. Intrabar-only fields such as the two-second FAST hold and final-three-second pre-close window are explicitly shown as unavailable in confirmed-bar replay.

For live monitoring with Kite market data and Nautilus Sandbox fills only:

```bash
./target/debug/kite-node native-trend-ribbon-dashboard-live \
  config/production-trend-ribbon.json 3600
```

The live dashboard uses the same Trend Ribbon actor/realtime engine as paper execution, shows actual Sandbox fill prices in the ledger, and never creates a Kite broker execution client. `Ctrl-C` performs the normal graceful stop. The header explicitly reports `PAPER / NO BROKER ORDERS`.

For deterministic terminal/screenshot verification without interactive raw mode:

```bash
./target/debug/kite-node native-trend-ribbon-dashboard-snapshot \
  config/production-trend-ribbon.json 2026-09-22
```

For a daily performance summary across a week or month:

```bash
./target/debug/kite-node native-trend-ribbon-dashboard-summary \
  config/production-trend-ribbon.json 2026-09-22 2026-09-25
```

Use Up/Down to select a trading day and Enter to open that day's detailed trade ledger. Home/End jumps to the first/last day and `q` exits. The range loader fetches Kite history in 30-day chunks with a 7-day indicator warmup, then replays the strategy once across the complete period so indicator state remains continuous.

A non-interactive summary snapshot is also available:

```bash
./target/debug/kite-node native-trend-ribbon-dashboard-summary-snapshot \
  config/production-trend-ribbon.json 2026-09-22 2026-09-25
```

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
apps/kite-node/tests/fixtures/ current Trend Ribbon regression fixture
vendor/               pinned NautilusTrader 0.63.0 patches
```

Generated build output, backtest reports, logs and runtime captures are ignored
by Git and should not be committed.

NautilusTrader is pinned to 0.63.0 with local compatibility patches documented
in `vendor/README.md`.

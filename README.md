# Rust Nautilus + Zerodha Kite — Pure Squeeze Momentum

Native Rust/NautilusTrader project for **MCX Crude PURE Squeeze Momentum v2.28.3** using Zerodha Kite market data and execution infrastructure.

## Active strategy

There is one active strategy only: `squeeze_momentum_lazybear_v2283`.

The strategy keeps the supplied TradingView Pine v2.28.3 structure, with the active production profile optimized to **OPT45 / DB0**:

- BUY: SQZ momentum positive and strengthening for configured N bars (default 2)
- SELL / QLX: configured weakening persistence (2 bars) plus configured retracement toward zero (**active 45%**; supplied Pine baseline was 70%), or immediate zero cross
- SHORT: SQZ momentum negative and strengthening downward for configured N bars
- COVER / QSX: inverse weakening/retracement rule, or immediate zero cross
- one trade per momentum wave; no RB/RS same-wave re-entry
- maximum one action per confirmed candle
- no same-bar reversal
- strategy state and BUY/SELL/SHORT/COVER actions are confirmed-bar only
- dashboard SQZ diagnostics can preview the forming candle tick-by-tick
- entry session default: 09:00–23:15 Asia/Kolkata
- day-end auto square-off default: ON at the candle closing 23:15
- safety hardening: the 23:10–23:15 square-off candle can close an existing position but can never open a fresh BUY/SHORT
- Algomojo action vocabulary: BUY / SELL / SHORT / COVER
- real orders remain disabled in the committed configuration

The source of truth is `config/production-squeeze-momentum.json`.

## JSON inputs

All active strategy/display inputs are JSON based under `squeeze_momentum`:

```json
{
  "sqz_length": 20,
  "sqz_length_kc": 20,
  "sqz_mult_kc": 1.5,
  "sqz_use_true_range": true,
  "entry_strength_bars": 2,
  "sqz_entry_deadband": 0.0,
  "sqz_dynamic_deadband_ema_length": 25,
  "sqz_dynamic_deadband_pct": 0.0,
  "sqz_weak_bars_req": 2,
  "sqz_transition_pct": 45.0,
  "session_timezone": "Asia/Kolkata",
  "allow_entries_only_in_session": true,
  "force_flat_at_session_end": true,
  "auto_sq_off_hour": 23,
  "auto_sq_off_minute": 15
}
```

The BB deviation intentionally uses `sqz_mult_kc`, matching the supplied Pine code. The supplied Pine baseline used a 70% transition exit; the active production profile uses the tested OPT45 value of 45%. `sqz_entry_deadband=0.0` preserves the original entry behavior; positive values are an optional entry-only anti-chop filter and do not change wave reset or exit logic. Dynamic wave deadband is disabled in production with `sqz_dynamic_deadband_pct=0.0`. When enabled, a new wave freezes `max(fixed_deadband, prior EMA(|SQZ|) × dynamic_pct)` until SQZ crosses zero; the reference EMA is updated only after each confirmed bar.

## Build and verify

```bash
cargo build --locked -p kite-node
./deploy/verify-squeeze-momentum-v2283.sh
```

The verifier runs formatting, strict Clippy, the Rust test suite, a deterministic historical fixture and a Nautilus Sandbox run. It also asserts that sandbox signals are emitted only after confirmed bar close and that there is at most one action per candle.

## Historical dashboard

Single day, interactive:

```bash
./target/debug/kite-node native-squeeze-momentum-dashboard-history \
  config/production-squeeze-momentum.json 2026-09-25
```

Single day, snapshot:

```bash
./target/debug/kite-node native-squeeze-momentum-dashboard-snapshot \
  config/production-squeeze-momentum.json 2026-09-25
```

Date-range summary:

```bash
./target/debug/kite-node native-squeeze-momentum-dashboard-summary-snapshot \
  config/production-squeeze-momentum.json 2026-09-22 2026-09-25
```

The dashboard shows SQZ momentum, positive/negative and rising/falling state, squeeze state, wave used/ready, entry readiness, strengthening count, weakening count, retracement %, tracked extreme, position, session state, day-end safety and last event.

## Live paper dashboard

```bash
./target/debug/kite-node native-squeeze-momentum-dashboard-live \
  config/production-squeeze-momentum.json 3600
```

The forming-candle dashboard updates tick-by-tick. Trading state is not mutated by those previews; orders/webhooks are generated only from completed candles.

## Record market data

```bash
./deploy/record-squeeze-momentum-ticks.sh 3600
```

The recorder creates no strategy and no execution client.

## Production

Build the live-capable binary only when intentionally needed:

```bash
cargo build --locked --release -p kite-node --features kite-adapter/live-orders
```

Launch manually with:

```bash
bash deploy/run-squeeze-momentum-live.sh
```

The committed `production-squeeze-momentum.json` keeps `live_orders_enabled=false`; real execution therefore remains gated until deliberately enabled and broker preflight/reconciliation passes.

## Kite authentication

```bash
./target/debug/kite-node native-kite-auth-login-url
./target/debug/kite-node native-kite-auth
```

The token utility updates the Redis access token without printing secrets.

## Repository layout

```text
apps/kite-node/src/native_node/squeeze_momentum_indicator.rs  LazyBear SQZ math
apps/kite-node/src/native_node/squeeze_momentum_strategy.rs   confirmed-bar state machine + tick preview
apps/kite-node/src/native_node/squeeze_momentum_backtest.rs   historical simulator
apps/kite-node/src/native_node/squeeze_momentum_actor.rs      Nautilus strategy actor
apps/kite-node/src/native_node/dashboard.rs                   historical/live SQZ dashboard
config/production-squeeze-momentum.json                       active JSON source of truth
deploy/verify-squeeze-momentum-v2283.sh                       validation
doc/SqueezeMomentumV2283.md                                   strategy notes
```

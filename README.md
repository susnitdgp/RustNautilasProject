# Rust Nautilus + Zerodha Kite — Smart Money Breakout Channels

Native Rust/NautilusTrader project for **MCX Crude Smart Money Breakout Channels v1.7** using Zerodha Kite market data and the existing Redis-backed execution/recovery infrastructure.

## Active strategy

There is one active strategy: `smart_money_breakout_channels_v17`.

Source of truth: `config/production-smbc.json`.

The profile currently uses the Pine v1.7 defaults: 100-bar normalization, 14-bar channel detection, strong closes, Wick + ATR Buffer stops, 4–15 point stop clamp, 1.5R target capped at 20 points, breakeven at 0.75R, ATR trail 1.2 activated at 0.75R, and 09:00–23:15 IST entry session.

Live order routing is intentionally disabled in the strategy JSON during validation.

## NautilusTrader

The workspace is pinned to NautilusTrader Rust crates `0.64.0` and Rust `1.98.1`. The prior local `0.63.0` crate overrides are no longer active.

## Verify

```bash
./deploy/verify-smbc-v17.sh
```

## Safe simulation

```bash
cargo build --locked -p kite-node
./target/debug/kite-node native-smbc-sim config/production-smbc.json
```

## Read-only Kite account check

```bash
./target/debug/kite-node native-kite-margins-check config/kite-production.json
```

## Production launcher

```bash
cargo build --locked --release -p kite-node --features kite-adapter/live-orders
bash deploy/run-smbc-live.sh
```

Real execution still requires both the strategy JSON gate and private broker JSON gate to be enabled, plus existing preflight/reconciliation controls.

See `doc/SMBCv17.md` for strategy details.

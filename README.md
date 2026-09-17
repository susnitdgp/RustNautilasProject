# Rust Nautilus + Zerodha Kite

Rust trading workspace using NautilusTrader, Kite market data and execution, and Redis persistence.

## Current setup

The execution code is restored to **v1.0.0 (`eedbd6b`)**, with a subsequent documentation cleanup. Local broker configuration and the release binary are managed separately from Git.

- Instrument: CRUDEOIL26SEPFUT; five-minute completed candles; one lot.
- Entries: Supertrend ATR(7), Wilder smoothing, multiplier 2; matching MACD(12,26,9) and session VWAP confirmation.
- Exits: opposite Supertrend, session shutdown or graceful stop. Additional ATR stop disabled.
- Production orders: MARKET / MIS / DAY with `market_protection=-1`.
- Quotes: Kite WebSocket. Order confirmation: REST polling, with fills confirmed from broker trades.
- Orders, application state and ownership: Redis. Failed runs require review before restart.

The September contract and configured calendar end September 21, 2026. Rollover is manual.

## Build and manual operation

From the repository root, build the live-capable binary:

```bash
cargo build --locked --release -p kite-node --features kite-adapter/live-orders -j 3
```

Real trading requires the live-orders build feature, enabled local broker configuration, the expected Kite account, valid credentials and clean ownership checks. The local production configuration may differ from the committed defaults.

The following command **starts real trading** when those gates pass; use it only when intending to trade:

```bash
./deploy/run-supertrend-live.sh
```

Keep the terminal open. Ctrl-C requests graceful shutdown and a reducing exit; verify the actual final position and open orders in Kite. Session mode must start during the configured trading hours and before its shutdown window. The application begins shutdown 30 minutes before the configured session close.

## Recovery

Reports are saved under `data/supertrend-live/<RUN_UUID>/`. Inspect a failed run without submitting orders:

```bash
./target/release/kite-node native-recover RUN_UUID
./target/release/kite-node native-kite-review RUN_UUID
./target/release/kite-node native-kite-status EXPECTED_USER_ID
```

Compare the journal with Kite orders, trades and positions before releasing stale ownership. A manual broker closure does not automatically update the old strategy journal. Preserve the reports and review audit; never clear Redis broadly to bypass startup checks.

## Maintained references

- [Redis keys, ownership and recovery](doc/RedisReference.md)
- [Redis credentials](doc/RedisCredentials.md)
- [Optional Slack alerts](doc/SlackAlerts.md)
- [Pinned Nautilus compatibility patches](vendor/README.md)

Historical implementation notes and verification reports have been removed from the working tree; earlier revisions remain in Git. The documentation cleanup does not change trading code or validate new live execution.

Nautilus is pinned to 0.63.0 with documented local patches. Its dependencies include LGPL-3.0-only components; review the upstream licenses before redistribution.

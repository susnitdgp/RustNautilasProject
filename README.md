# Rust Nautilus + Zerodha Kite

Rust trading workspace using NautilusTrader, Kite market data and execution, and Redis persistence.

## Current setup

The working tree builds on **v1.0.0 (`eedbd6b`)**, with shutdown reporting fixes and WebSocket-triggered order reconciliation. Local broker configuration and the release binary are managed separately from Git.

- Instrument: CRUDEOIL26OCTFUT (expiry October 19, 2026); five-minute completed candles; one lot.
- Entries: Supertrend ATR(7), Wilder smoothing, multiplier 2; matching MACD(12,26,9) and session VWAP confirmation.
- Exits: opposite Supertrend, session shutdown or graceful stop. Additional ATR stop disabled.
- Production orders: MARKET / MIS / DAY with `market_protection=-1`.
- Quotes: Kite WebSocket. Production order updates: a dedicated Kite order WebSocket triggers REST reconciliation; slower fallback and pending checks remain, with fills confirmed from broker trades.
- Orders, application state and ownership: Redis. Failed runs require review before restart.

Contract selection and live session coverage are configured in `config/production-supertrend.json`. The October symbol, token `145894407`, expiry `2026-10-19`, tick size and broker lot size were checked against the [Kite MCX instrument master](https://api.kite.trade/instruments/MCX) on September 22, 2026. Rollover remains manual.

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

Sandbox strategy lifecycle hooks are disabled by default. To opt in, set `sandbox_webhooks.enabled` to `true` in `config/kite-production.json`; the sandbox runner then POSTs `{"action":"start","mode":"sandbox"}` for both configured strategies before the run and `{"action":"stop"}` for both after the node run ends. A failed start/stop is reported for review, redirects are rejected, and production never uses these hooks. The default command is `./target/release/kite-node native-kite-sandbox`; an alternate webhook config can be supplied as its third argument.

## Contract rollover through JSON

For the live Supertrend launcher and its paper/mock variants, `config/production-supertrend.json` is the contract source of truth:

```json
{
  "instrument": "CRUDEOIL26OCTFUT.MCX",
  "symbol": "CRUDEOIL26OCTFUT",
  "instrument_token": 145894407,
  "expected_expiry": "2026-10-19",
  "session_calendar": {
    "timezone": "Asia/Kolkata",
    "valid_from": "2026-08-17",
    "valid_through": "2026-10-19",
    "regular": { "open": "09:00:00", "close": "23:30:00" },
    "overrides": {
      "2026-09-14": { "open": "17:00:00", "close": "23:30:00" },
      "2026-10-02": null
    }
  }
}
```

This is the contract/calendar portion of the file; retain its strategy fields. At the next rollover, stop the node, review the prior contract's positions and orders, then update the exact symbol, instrument ID, token and expiry from the current Kite master. Review calendar coverage, hours and holiday overrides for the new period, including at least the preceding seven calendar days for warmup. `null` marks a fully closed holiday; an hours object marks a shortened session. Weekends are closed, and dates outside the configured range fail validation. The October 2 closure follows [Zerodha's 2026 MCX holiday calendar](https://zerodha.com/marketintel/holiday-calendar/).

The same JSON calendar controls live startup, the existing 30-minute exit buffer, warmup validation and corrected-history recovery. No contract is automatically selected at expiry. The broker token in `config/kite-production.json` is overridden by this selection for Supertrend; account, product, live-order gate and webhook settings remain in the broker file. Legacy September historical replay/crossover fixtures are separate from this live selection.

Check the selected contract and calendar without starting trading, reading account credentials or calling webhooks:

```bash
./target/release/kite-node native-contract-check config/production-supertrend.json
```

The check verifies the exact symbol/token/expiry against Kite and validates the supported tick/lot specification. It is not an account, funds or complete trading-readiness check. JSON changes are read at startup; future rollovers do not require a Rust rebuild. Restart manually only after reviewing the check and account state. The launcher command remains unchanged.

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

Historical implementation notes and verification reports have been removed from the working tree; earlier revisions remain in Git. Automated tests do not establish live broker or external webhook qualification.

Nautilus is pinned to 0.63.0 with documented local patches. Its dependencies include LGPL-3.0-only components; review the upstream licenses before redistribution.

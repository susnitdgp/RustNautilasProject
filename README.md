# Rust Nautilus + Zerodha Kite

Rust trading workspace using NautilusTrader, Kite market data and execution, and Redis persistence.

## Trend Ribbon configuration layout

The Trend Ribbon live launcher reads `config/production-trend-ribbon.json` and the private `config/kite-production.json`. The strategy JSON `interval` field is the source of truth for three- versus five-minute Trend Ribbon bars. Other JSON presets have moved to `config/backup/`; their contents and live-order flags are unchanged. The optional launchers, source defaults and tests reference the new locations. Existing TOML files remain in place. See [configuration guide](config/README.md). This is a file-layout change, not a strategy or safety fix.

## Current setup

**Release v2.0.0** packages the current Trend Ribbon and Pivot implementations, WebSocket-triggered order reconciliation, engineering documentation, and the reorganized configuration layout. The `kite-node` application package is version `2.0.0`; internal library crate and pinned Nautilus versions are unchanged. Local broker configuration and release artifacts are managed separately from Git. See [v2.0.0 release notes](doc/releases/v2.0.0.md).

- Instrument: CRUDEOIL26OCTFUT (expiry October 19, 2026); JSON-selected completed candles; one lot. The active Trend Ribbon selection currently uses five-minute candles; three-minute remains supported through JSON.
- Active selection: Trend Ribbon [BOSWaves], ALMA(34, 0.85, 6), deviation(34) x 0.65, ATR(14), 3-bar slope threshold 0.08. The original Supertrend/MACD/VWAP and Pivot presets remain available under `config/backup/`.
- Exits: opposite selected trend, session shutdown or graceful stop. Additional ATR stop disabled. Trend Ribbon has the documented quote-outage square-off limitation below.
- Production orders: MARKET / MIS / DAY with `market_protection=-1`.
- Quotes: Kite WebSocket. Production order updates: a dedicated Kite order WebSocket triggers REST reconciliation; slower fallback and pending checks remain, with fills confirmed from broker trades.
- Orders, application state and ownership: Redis. Failed runs require review before restart.

Contract selection and live session coverage use `config/production-trend-ribbon.json` for Trend Ribbon, `config/backup/production-supertrend.json` for the original strategy and `config/backup/production-pivot-supertrend.json` for Pivot Point SuperTrend. The October symbol, token `145894407`, expiry `2026-10-19`, tick size and broker lot size were checked against the [Kite MCX instrument master](https://api.kite.trade/instruments/MCX) on September 22, 2026. Rollover remains manual.

**Release qualification:** this is a versioned engineering snapshot, not approval for unattended live trading. Trend Ribbon still differs from the uploaded Pine at mandatory square-off (`trend := 0` is not implemented), and its dedicated quote-independent square-off timer is missing. The pinned 17-flip regression is not exact TradingView parity proof. Real broker fills and outage-at-cutoff behavior remain unqualified. These issues are documented, not fixed, in v2.0.0. See [engineering findings](doc/EngineeringDesignAndStrategies.md).

## Build and manual operation

From the repository root, build the live-capable binary:

```bash
cargo build --locked --release -p kite-node --features kite-adapter/live-orders -j 3
```

Real trading requires the live-orders build feature, enabled local broker configuration, the expected Kite account, valid credentials and clean ownership checks. The local production configuration may differ from the committed defaults.

The following command **starts real trading** when those gates pass; use it only when intending to trade:

```bash
./deploy/run-trend-ribbon-live.sh
```

Keep the terminal open. Ctrl-C requests graceful shutdown and a reducing exit; verify the actual final position and open orders in Kite. Session mode must start during the configured trading hours and before its shutdown window. The application begins shutdown 30 minutes before the configured session close.

Sandbox strategy lifecycle hooks are disabled by default. To opt in, set `sandbox_webhooks.enabled` to `true` in `config/kite-production.json`; the sandbox runner then POSTs `{"action":"start","mode":"sandbox"}` for both configured strategies before the run and `{"action":"stop"}` for both after the node run ends. A failed start/stop is reported for review, redirects are rejected, and production never uses these hooks. The default command is `./target/release/kite-node native-kite-sandbox`; an alternate webhook config can be supplied as its third argument.

## Contract rollover through JSON

The selected strategy JSON is the contract source of truth: `config/production-trend-ribbon.json` for live Trend Ribbon, `config/backup/production-supertrend.json` for the original live strategy, `config/backup/production-pivot-supertrend.json` for live Pivot, and `config/backup/pivot-point-supertrend.json` for Pivot paper/simulation. Update each selection that you intend to run at rollover. Example identity/calendar fields:

```json
{
  "instrument": "CRUDEOIL26OCTFUT.MCX",
  "symbol": "CRUDEOIL26OCTFUT",
  "instrument_token": 145894407,
  "interval": "5minute",
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

The same JSON interval and calendar control historical API requests, Nautilus bar type, completed-bar timing, warmup validation, corrected-history recovery, dashboard countdown, live startup and the existing 30-minute exit buffer. No contract is automatically selected at expiry. The broker token in `config/kite-production.json` is overridden by this selection for Supertrend; account, product, live-order gate and webhook settings remain in the broker file. Legacy September historical replay/crossover fixtures are separate from this live selection.

Check the selected contract and calendar without starting trading, reading account credentials or calling webhooks:

```bash
./target/release/kite-node native-contract-check config/production-trend-ribbon.json
```

The check verifies the exact symbol/token/expiry against Kite and validates the supported tick/lot specification. It is not an account, funds or complete trading-readiness check. JSON changes are read at startup; future rollovers do not require a Rust rebuild. Restart manually only after reviewing the check and account state. The launcher command remains unchanged.

## Order reconciliation

Production uses the dedicated Kite order WebSocket to trigger authoritative REST reads. Notifications and HTTP acknowledgements do not create fills. A 15-second fallback and a 5-second unresolved-order check recover missed notifications; these intervals are currently code defaults, not JSON settings.

Orders, trades and positions must agree before fill events are persisted and published. Temporary order/trade or pending-order position disagreement shares the existing three-attempt read budget with transient transport failures (250 ms then 500 ms backoff, 12 seconds per snapshot attempt). Identity errors, duplicate/changed trades, authentication failures and rate limits are not retried. Persistent disagreement stops trading with `ReviewRequired`; order submissions are never retried automatically.

New entries remain blocked during WebSocket recovery until a fresh reconciliation succeeds. Existing order deadlines and shutdown review gates still apply; retries do not extend them. The same validation runs during final shutdown reconciliation. Unit and mock tests cover these paths, but a successful no-order live session does not qualify real fill handling.

## Pivot Point SuperTrend

The supplied intraday Pine indicator is available as `pivot_point_supertrend`, configured separately in `config/backup/pivot-point-supertrend.json`: pivot period 2, ATR(10) × 3, Monday–Friday 09:00–23:15 IST, daily session reset, and entries only on fresh trend flips. It uses no MACD/VWAP filter. Reversals close the current position before entering the opposite side; a native clock callback requests session square-off.

```bash
./target/release/kite-node native-pivot-kite-mock config/backup/pivot-point-supertrend.json
```

This command uses synthetic data and the native Kite mock adapter. For live-data paper operation use `native-pivot-session-paper` with the same JSON.

Pivot production uses a separate selection, `config/backup/production-pivot-supertrend.json`, and a manual launcher. Its strategy gate is enabled in that file; the live-capable build and enabled private broker configuration are still required. The paper selection cannot start production.

```bash
# Offline configuration check; does not start trading or access the account.
./target/release/kite-node native-pivot-production-check \
    config/backup/production-pivot-supertrend.json config/kite-production.json

# Starts REAL Pivot Point trading after the runtime gates pass.
./deploy/run-pivot-live.sh
```

The production default is 09:00–23:00 IST. The effective cutoff is capped at 30 minutes before the configured market close; paper retains 23:15. Production uses the same Kite adapter, WebSocket-triggered reconciliation, reducing exits and Redis account guards as the original strategy. The original Supertrend launcher retains its selection. See [parameters, production procedure, Pine session corrections and run modes](doc/PivotPointSupertrend.md).

## Recovery

Reports are saved under `data/supertrend-live/<RUN_UUID>/`. Inspect a failed run without submitting orders:

```bash
./target/release/kite-node native-recover RUN_UUID
./target/release/kite-node native-kite-review RUN_UUID
./target/release/kite-node native-kite-status EXPECTED_USER_ID
```

A retained account owner is checked before a new strategy lease is created. A blocked restart reports the retained owner for review; it does not automatically unlock the account. If initialization or reconciliation fails before counts can be verified, the report uses `null` for open orders and position.

Compare the journal with Kite orders, trades and positions before releasing stale ownership. A manual broker closure does not automatically update the old strategy journal. Preserve the reports and review audit; never clear Redis broadly to bypass startup checks.

## Maintained references

- [Redis keys, ownership and recovery](doc/RedisReference.md)
- [Redis credentials](doc/RedisCredentials.md)
- [Optional Slack alerts](doc/SlackAlerts.md)
- [Pinned Nautilus compatibility patches](vendor/README.md)

Historical implementation notes and verification reports have been removed from the working tree; earlier revisions remain in Git. Automated tests do not establish live broker or external webhook qualification.

Nautilus is pinned to 0.63.0 with documented local patches. Its dependencies include LGPL-3.0-only components; review the upstream licenses before redistribution.

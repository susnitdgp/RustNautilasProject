# Pivot Point SuperTrend — Rust Nautilus port

This selectable strategy ports the supplied **Pivot Point SuperTrend [Intraday - Crude Oil]** Pine indicator into the existing Nautilus bar strategy and execution path. The source-derived indicator retains the MPL-2.0 notice and attribution to LonesomeTheBlue.

Paper and simulation use `config/backup/pivot-point-supertrend.json`. Manual production uses `config/backup/production-pivot-supertrend.json` with `deploy/run-pivot-live.sh`. The original `deploy/run-supertrend-live.sh` continues to select Supertrend + MACD/VWAP in `config/backup/production-supertrend.json`. Both production strategies share the existing native Kite execution adapter, account ownership, Redis journal, order WebSocket and authoritative REST reconciliation.

## Parameters and behavior

| Setting | Default |
|---|---|
| Contract | CRUDEOIL26OCTFUT.MCX, token 145894407 |
| Candle interval | Completed five-minute candles |
| Pivot left/right period | 2 bars on each side |
| ATR | Period 10; Wilder RMA with arithmetic-mean seed |
| ATR factor | 3 |
| Position size | One contract |
| Paper session | 09:00–23:15 Asia/Kolkata |
| Production session | 09:00–23:00 Asia/Kolkata by default |
| Session days | `23456` (Monday–Friday) |
| Daily reset | Enabled |
| Entry | Fresh SuperTrend -1→+1 or +1→-1 transition |
| Exit | Opposite transition, configured session deadline, or shutdown |
| Additional filters/stops | No MACD/VWAP filter, separate stop order, or target |

A pivot is available only after its right-hand bars have closed. There are no orders dated back to the pivot candle. On equal extremes the rightmost candidate wins. If a high and low pivot confirm together, the high has priority in the center update, matching the supplied conditional expression.

The first pivot initializes the center. Later pivots apply `(2 × previous_center + new_pivot) / 3`. The bands are center ± factor × ATR; the trailing-band recurrence and trend comparisons use previous-bar values. ATR uses the mean of the first N true ranges, followed by Wilder smoothing, including gaps from the previous close. It does not use the older strategy's first-true-range seed.

Like the supplied script's `Trend = 0`, a fresh session starts neutral. Establishing the first direction is not an entry signal: entry requires a subsequent transition between -1 and +1. Historical warmup never places an entry. A history rebuild may require an exit but cannot replay an old entry. A flat position remains flat after recovery (including a cached total of `-0.0`); only an actual nonzero position that still agrees with the rebuilt trend is retained.

Nautilus tracks the actual position and order fills. A reversal submits a reducing exit first, waits for confirmation, then may enter the opposite side. Submission acknowledgements are not fills. Existing persistence, ownership, reconciliation and order-deadline checks remain in force.

## Explicit corrections to the Pine session behavior

The supplied script resets variables after computing bands and then refers to `[1]`, which can restore prior-session values. Its session transition also relies on an out-of-session bar, and square-off labels infer the position from trend. The Rust port implements the intended intraday behavior:

- A date change starts a new session even when the chart/feed omits all overnight bars.
- Reset occurs before pivot-center/band processing. It clears the pivot window, center, trailing bands, trend, and support/resistance. No previous-session pivot can seed the reset window. ATR remains continuous, as the independent `ta.atr` series does in Pine.
- The JSON market calendar still applies: a holiday remains closed and a late market open shortens the trading window. The configured strategy session can never extend outside market hours.
- New entries are blocked at the configured session boundary, including signals from the candle closing at that boundary. Production additionally caps the deadline at 30 minutes before the JSON market close, even if a later strategy end is configured.
- A native clock callback checks the deadline every 250 ms and requests an exit for the actual open position without waiting for an out-of-session bar or a fresh quote. A pending order is resolved first. A square-off request is not a guarantee of a fill; unresolved execution remains `ReviewRequired`.

These deliberate corrections mean this is not a claim of bar-for-bar equivalence to the buggy session-reset behavior. TradingView CSV parity and real broker fill qualification have not been performed. Mock execution and configuration checks do not establish real broker qualification.

## Run manually

From `/home/ubuntu/RustNautilasProject`:

```bash
# Synthetic candles and Nautilus simulated orders.
./target/release/kite-node native-pivot-sim config/backup/pivot-point-supertrend.json

# Synthetic candles through the native Kite mock adapter; no broker network/orders.
./target/release/kite-node native-pivot-kite-mock config/backup/pivot-point-supertrend.json

# Live Kite quotes and history, simulated execution, until the session deadline.
./target/release/kite-node native-pivot-session-paper config/backup/pivot-point-supertrend.json
```

Live-data paper mode requires valid data credentials and a supported trading day, and must be started inside the configured strategy session. It does not enable broker orders. Simulation creates ordinary isolated run records in Redis; automated integration tests provision a separate temporary Redis server.

## Manual production

`config/backup/production-pivot-supertrend.json` explicitly enables Pivot's strategy gate with `live_orders_enabled: true`. This file alone never starts a node. Production additionally requires the `kite-adapter/live-orders` build feature, `live_orders_enabled: true` in the local `config/kite-production.json`, a valid expected account, MIS product, automatic market protection (`-1`), valid data credentials, matching instrument metadata and clean ownership/account admission. The private broker file is not rewritten. Its instrument token is overridden by the selected strategy JSON, as for the existing strategy.

```bash
cargo build --locked --release -p kite-node --features kite-adapter/live-orders -j 3

# Offline configuration/build-gate check: no credentials, Redis or broker requests.
./target/release/kite-node native-pivot-production-check \
  config/backup/production-pivot-supertrend.json config/kite-production.json

# Read-only instrument master check; no orders or account access.
./target/release/kite-node native-contract-check config/backup/production-pivot-supertrend.json

# STARTS REAL TRADING when the runtime gates pass.
./deploy/run-pivot-live.sh
```

Equivalent direct command:

```bash
./target/release/kite-node native-pivot-kite-production \
  config/backup/production-pivot-supertrend.json config/kite-production.json
```

Stop any paper session manually before switching the terminal to production, and review its final report. Keep the production terminal open. Ctrl-C requests a reducing exit, waits for execution, and performs final broker reconciliation; check the final position in Kite. No launcher rebuilds, clears Redis ownership, changes configs or restarts automatically.

The production cutoff is the earlier of the configured strategy end and the calendar's market close minus the existing 30-minute MIS application buffer. The default calendar closes at 23:30, so production requests square-off at 23:00 IST. This is an application policy, not a broker execution guarantee. Calendar overrides also apply. The dashboard displays the effective live cutoff and reports include `square_off_ns`. Startup requires more than 15 seconds remaining before that cutoff. Paper retains its separate 23:15 cutoff.

Use the production selection with `native-pivot-kite-mock` to exercise the native adapter without real orders. Explicit paper/mock commands always select simulated execution even if the input selection's production gate is enabled. Conversely, the paper selection (`live_orders_enabled: false`) cannot activate the production command.

Order acknowledgements and WebSocket notifications do not generate fills directly. The shared adapter confirms consistent orders, trades and positions before publishing fills, blocks new entries during reconciliation recovery, and leaves unresolved exposure/orders in `ReviewRequired`. Order submissions are never retried automatically. A strategy trend flip first closes the existing position, then may enter the other side after fill confirmation. Historical warmup/recovery never authorizes a new entry.

Settings are loaded on startup. The parameter object is:

```json
"pivot_point": {
  "pivot_period": 2,
  "atr_period": 10,
  "atr_factor": 3.0,
  "session": {
    "start": "09:00:00",
    "end": "23:15:00",
    "days": "23456",
    "reset_daily": true
  }
}
```

The current runner supports a single same-day, five-minute-aligned IST window. Invalid configurations are rejected. The session-day numbering follows [TradingView's session format](https://www.tradingview.com/pine-script-docs/concepts/sessions/).

Reports continue under `data/supertrend-live/<RUN_UUID>/`, identified by `strategy: pivot_point_supertrend`. `indicators.json` records confirmed pivots, center, ATR, support/resistance, trend, initialization and session flags. `signals.json` and `fills.json` record native decisions and executions. Plot visibility inputs from Pine are visual-only; Rust records these values rather than drawing TradingView shapes or backgrounds.

Implementation modules: `pivot_point.rs` (indicator), `pivot_session.rs` (session policy), `pivot_production.rs` (offline production check), the shared `supertrend_actor.rs` (native execution), and `supertrend_live_runner.rs` (node wiring).

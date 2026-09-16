# Selected strategy: session operation and production review

## Implemented selection

CRUDEOIL26SEPFUT, five-minute Supertrend(7,2) + native MACD components (12,26,9) + session HLC3 VWAP, one lot. No added ATR stop. Long/short entries, reducing exits, and a shutdown before session end. The September contract/calendar expires September 21, 2026; rollover is not automatic.

The selected strategy submits **MARKET/DAY** orders. Native Kite translation always includes **market_protection=-1** (automatic protection), without a limit price. Both entry and reducing exit use that policy. Zero/unprotected requests are rejected. Kite may internally convert the protected market order to a limit; reconciliation accepts the owned conversion only with reported protection. Fills must still come from broker trades, never an acknowledgement. Protected orders can remain unfilled or be rejected by exchange price limits. See https://kite.trade/docs/connect/v3/orders/#market-protection.

## Commands from the project directory

```bash
cd /home/ubuntu/RustNautilasProject
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked --release -p kite-node -j 3
# Native dispatcher / market-protection integration without broker access:
./target/release/kite-node native-supertrend-kite-mock config/production-supertrend.json
# Full remaining session using live Kite data and Nautilus simulated orders:
./target/release/kite-node native-supertrend-session-paper config/production-supertrend.json
# Shorter bounded paper check:
./target/release/kite-node native-supertrend-paper config/production-supertrend.json 360
```

Session mode must start during the supported trading session. It begins graceful shutdown 60 seconds before the calendar close, leaving time to handle an exit. It does not start again the next day. Bounded paper runs accept 5–86360 seconds; use session mode to enforce the market-close deadline.

## Recovery, ownership and stops

A quote disconnect pauses trading decisions. A successful reconnect alone does not resume entries: checked historical coverage, an indicator rebuild and fresh quotes are required. OHLCV revisions rebuild indicators from corrected history; prior orders/signals are not replayed. OI-only changes do not alter these indicators. Missing candles/read errors pause admission and retry up to six polling attempts; exhausted retries stop the run. The quote supervisor permits two reconnects before declaring failure.

Redis ownership is checked every five seconds off the trading thread. Missing ownership or unavailable Redis stops the run. Crashed/unclean runs retain durable ownership and native orders/positions. No automatic adoption or resubmission is attempted after restart. Inspect first:

```bash
./target/release/kite-node native-recover RUN_UUID
./target/release/kite-node native-kite-status EXPECTED_USER_ID
./target/release/kite-node native-kite-review RUN_UUID
```

Compare native state with the broker's orders, trades and positions before releasing a production ownership record. The software deliberately refuses an automatic restart with unresolved state. Keep recovery records; never clear all Redis keys to make startup work. Production startup verifies the configured Kite identity and requires an account without existing positions or unresolved orders.

Orders have a ten-second fill deadline. An unresolved order stops the strategy and triggers cancellation during shutdown, with no blind repricing or duplicate submission. Graceful stop attempts a reducing exit while fresh data and execution remain available. If an exit cannot be confirmed, the run and account remain ReviewRequired; a market-protection parameter does not guarantee flattening. SIGTERM/Ctrl-C are handled before stopping the hosted LiveNode.

## Broker gate (not activated)

config/kite-production.json explicitly contains live_orders_enabled=false, market_protection=-1 and an unset expected account ID. Production factory requires both a separately reviewed build with kite-adapter/live-orders enabled and an explicitly enabled broker configuration. The ordinary release build cannot submit real orders. No enabled broker configuration is delivered.

The code entry point for later controlled broker validation is native-supertrend-kite-production with the strategy selection and broker-settings paths. This stage still needs manual source review and separately authorized broker validation. Do not start it merely to check connectivity. The old native-node-live command remains disabled; the selected strategy has its own explicit entry point.

Files: strategy supertrend_actor.rs; session/deadline supertrend_session.rs; LiveNode assembly supertrend_live_runner.rs; corrected history supertrend_revision.rs; native broker gate crates/kite-adapter/src/execution/native_client/production.rs; wire fields crates/kite-adapter/src/execution/request.rs; native translation execution/native.rs; order/fill reconciliation execution/broker_events.rs. Supertrend files are under apps/kite-node/src/native_node/.

## Manual launch selected; optional service reference

The user selected manual terminal launch on September 16; service installation is not a deployment requirement. Use the session-paper command above and see UnattendedValidation.md and SlackAlerts.md. The following service commands are optional reference only.

The reviewed service definition is deploy/kite-supertrend-paper.service. It runs as ubuntu, does not restart automatically, and accepts SIGTERM for graceful shutdown. The service uses the ordinary release binary (real-order feature disabled). No timer or boot activation is configured. Installation and starting during trading hours:

```bash
sudo install -m 0644 deploy/kite-supertrend-paper.service /etc/systemd/system/kite-supertrend-paper.service
sudo systemctl daemon-reload
sudo systemctl start kite-supertrend-paper.service
sudo systemctl status kite-supertrend-paper.service --no-pager
sudo journalctl -u kite-supertrend-paper.service -f
# Graceful stop:
sudo systemctl stop kite-supertrend-paper.service
```

Do not start a foreground paper node and this service together; the durable ownership gate prevents concurrent runs. If the service fails, inspect its journal, run report and native Redis reconstruction before another start. Do not add Restart=always or a daily timer before recovery procedures and contract rollover have been reviewed.

## Verification limits

Mock execution verifies protected-market request serialization, broker conversion, native fill/reconciliation mapping and durable event history. Nautilus paper execution does not emulate Kite's exact automatic protection thresholds, brokerage charges or exchange rejection rules. Real API fills, account admission and exchange behaviour still require the separately authorized broker-validation stage. A short forward run and passing tests do not substitute for full-session operational qualification.

## Verification checkpoint, September 16

Workspace run: 212 passing tests plus one new passing protected-market conversion/partial-cancellation test (213 total covered); one existing ignored fixture. Feature-enabled adapter gate: 109 passing tests. Selected-strategy native mock, Redis reconstruction, corrected-history replay and SIGTERM regressions passed. Formatting and Clippy with warnings denied checked. The shipped optimized binary is built without the live-orders feature.

Live-data paper run 55cb0796-3c95-4071-80da-eb2fbb05af57 lasted 360 seconds: 332 accepted quotes, 843 warmup plus one new completed candle, one successful indicator rebuild after a correction, two simulated fills, no open orders or position, Clean final status. This verifies recovery over a candle boundary and operation beyond the old 300-second feed limit. It is not a whole-session or real-broker qualification.

An older revision-stop run c83daa6e-0c44-4353-8d8f-b0c0b7661521 was reconstructed read-only from native Redis and confirmed flat before releasing its paper ownership. Its reports and health records were retained.

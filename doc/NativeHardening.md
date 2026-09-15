# Native hardening and strategy review

Updated 15 September 2026. This follows integration commit 84d2df2.
Real Kite orders remain disabled. The user authorized the official Kite sandbox,
manual code review and continued testing. No production-order activation is included.

## Strategy interface

Edit apps/kite-node/src/native_node/strategy.rs. UserStrategy.on_full_tick receives
KiteFullTick; on_quote handles quote-only backtests. Return an optional Signal:

| Signal | Required position | Broker side | Effect |
| --- | --- | --- | --- |
| BUY | Flat | BUY | Enter one long contract |
| BUY_EXIT | Long one | SELL, reduce-only | Close the long |
| SELL | Flat, short enabled | SELL | Enter one short contract |
| SELL_EXIT | Short one | BUY, reduce-only | Cover the short |

Signals live in native_node/signals.rs; the lifecycle/order adapter is actor.rs.
The default strategy still uses native 3/5-tick SMA crossovers. It exits an existing
position on the opposite crossover; it does not reverse in the same order.
max_entries counts both long and short entry attempts. Exits bypass this entry cap.
Only one contract in either direction and one pending order are permitted.
Native dispatch independently verifies the contract cap and reducing direction.

config/strategy-crossover.toml now enables short entries. It also has sample local
stop_loss_rupees=30 and target_rupees=60. These are quoted-price distances from the
native average entry price, not total INR account PnL. For the configured standard
100-barrel contract these distances imply INR 3000/6000 before fees and slippage.
These sample values are implementation defaults, not an optimized trading setup.

Protection uses the executable bid for a long exit and ask for a short cover.
A protective exit takes precedence over the crossover decision. Exits are checked
only on valid fresh ticks. Wide spreads block entries but do not block an otherwise
valid reducing exit. One pending order still prevents overlapping commands.

These are LOCAL tick-triggered LIMIT exits, not broker/exchange-held stop orders.
Feed loss, a stopped process, gaps, insufficient depth or a non-marketable resting
limit can delay/prevent filling. There is no MARKET/SL/SL-M/GTT fallback or automatic
repricing. Do not interpret a triggered stop as a guaranteed exit or maximum loss.
The older non-native strategy paths retain their previous long-only behavior.

## Account coordination and rates

execution/native_client/coordination.rs maintains one durable owner per account.
Redis key: susanta:nautilus:native-kite:account:{ACCOUNT}.
Mock uses MOCK; the official sandbox uses SB followed by the verified sandbox user.
Ownership covers every participating native client/process for that account.
A second process cannot acquire an occupied account. Ownership never expires:
there is no TTL takeover that could overlap an in-flight command after a crash.
The owner is checked before dispatch, in telemetry updates and during release.
Missing, replaced, expiring or uncertain Redis state blocks dispatch.

The existing rolling Redis rate limiter is reused with conservative application
limits: five commands/second, 100/minute and 1000/rolling 24 hours. Cancellations and
failed requests consume budget. A clean restart reopens the SAME budget; it does
not reset it. Missing budget state blocks startup. HTTP 429 applies a durable shared
cooldown of at least ten seconds and stops execution for review. No mutation retry.
This coordinates this application, not external scripts/manual orders using the
same account. Unowned open orders or unmanaged exposure stop native reconciliation.

Each ReadClient serializes and spaces requests by 150 ms. This is local pacing;
it is not an account-wide REST quota service for unrelated applications.
The real production factory still has no dispatcher, even with live-orders enabled.

## Outages, shutdown and monitoring

outage.rs retries only classified transient snapshots: at most three attempts,
12-second deadline per attempt and 250/500-ms backoff. Authentication, schema,
ownership and rate-limit errors are not automatically retried. Mutations have a
six-second deadline; uncertain results remain unresolved and are never resubmitted.
Redis has separate connect/read/write/AOF deadlines. Its synchronous calls can
occupy a worker until those deadlines; async cancellation cannot interrupt them.

Broker observations are checked against owned fills and account exposure before
publication. Polling updates the account heartbeat once per cycle. Exhausted reads,
Redis uncertainty, changed ownership or inconsistent exposure disable admission,
signal the node to stop and retain the account for review.
WebSocket heartbeat traffic cannot indefinitely mask missing fresh market data.
A ten-second usable-data watchdog produces a gap and uses the bounded reconnect
budget. Every reconnect retains the chosen production or sandbox host.

SIGINT/SIGTERM request native shutdown. New command admission stops first; queued
commands recheck the active flag after acquiring the dispatcher lock. Task drains
have 15-second deadlines, abort and join overdue async tasks, and report failures.
There is a 256-task admission cap. Full-tick capture is capped at 100000 packets.
Captured full packets are written even after a failed run when any were received.

Shutdown reconciles again. It releases ownership only after a verified flat account,
no unresolved orders and no task/persistence/reconciliation failure. Otherwise it
persists ReviewRequired and fails the run. Shutdown does not silently flatten an
open position or claim that a cancel acknowledgement is a cancellation.

    cargo run --locked -p kite-node -- native-kite-status MOCK
    cargo run --locked -p kite-node -- native-kite-review NAMESPACE
    cargo run --locked -p kite-node -- native-recover NAMESPACE

Status returns state, owner, last namespace, heartbeat age, position, unresolved
count, command attempts and review/restart flags. Clean stopped accounts do not
raise stale-heartbeat alarms. Review/recovery commands do not send broker orders.
Native execution faults and failed-node summaries emit structured redacted JSON.
Use these outputs for operator monitoring; no external alert delivery is configured.

## Restart procedure

1. For a normal stop, confirm state=Clean, owner empty, no unresolved orders and
   zero position. Start a NEW run namespace; budgets and cooldown remain intact.
2. After interruption, stop the old process and any process using that account.
   Keep the account key, command journal and native cache intact. Do not FLUSHDB,
   DEL ownership keys, introduce TTLs, or bypass startup checks to force a restart.
3. Run both read-only recovery commands. Compare journal events/ownership/exposure
   with the native cache. Review Dispatching/Unknown outcomes and pending cancels.
4. For Kite sandbox, renew sandbox authentication if necessary and compare the
   current sandbox order/trade/position book. A journal alone cannot prove the
   broker is flat. Resolve any sandbox exposure using reviewed sandbox procedures.
5. An unclean owner remains blocked for manual engineering review. There is NO
   automatic unlock, resume, stale-lock takeover or command replay command in this
   checkpoint. A reviewed reset/migration must retain evidence and rate budgets.

## Official Kite sandbox

    cargo run --locked -p kite-node -- native-kite-sandbox-preflight
    cargo run --locked -p kite-node -- native-kite-sandbox config/kite-sandbox.toml config/strategy-crossover.toml

Credentials are read only from sandbox:kite_api_key and sandbox:kite_access_token.
There is no fallback to susanta:* production credentials. HTTP uses fixed
https://sandbox.kite.trade/oms routes; WebSocket uses wss://ws-sandbox.kite.trade
with the verified user_id. Redirects are disabled. No arbitrary root URL setting.

After successful preflight, put its sandbox user_id into the expected_user_id field
in config/kite-sandbox.toml. The placeholder intentionally blocks order execution.
MCX, the configured product and CRUDEOIL26SEPFUT token must match preflight. Other
contracts are not silently substituted. Sandbox testing uses sandbox ticks, not
synthetic fixture prices. SandboxFactory is separate from the disabled real factory.

Kite sandbox supports LIMIT orders but not MARKET, GTT or virtual contract notes.
For native sandbox reports, fee estimates are explicitly zero and labelled as
sandbox estimates, not actual broker charges. Production reports retain their
existing calculated-charge validation. Current sandbox acceptance/fill behavior
still requires authenticated verification against the official service.

Read-only sandbox checks on 15 September returned HTTP 403 with the supplied Redis
credentials. No orders were sent. Refresh/verify sandbox credentials before testing;
no credential values were logged or changed, and no real credentials were used.

## Full tick / order-flow review

    cargo run --locked -p kite-node -- native-full-audit CATALOG_PATH

The strategy receives tick.snapshot.raw as a complete decoded Kite Tick, including:
- ltp_paise, last_quantity and cumulative_volume;
- quote_fields: average price, total buy/sell quantities and OHLC;
- full: OI, OI day high/low, last-trade and exchange timestamps;
- full.bids[0..5] and full.asks[0..5], each with price_paise, quantity and orders;
- snapshot receipt time, instrument token, source freshness and connection generation;
- tick.quote: native bid/ask prices, sizes and native timestamps.

Raw price fields use paise; the configured crude-oil quote uses rupees. Full data is
kept in native CustomData and persisted in Parquet; equality checks cover the whole
packet and both native timestamps. The catalog test assigns distinct depth/order
counts and OI fields so loss of deeper fields cannot pass as a top-of-book test.
Full data reaches on_full_tick in live/sandbox/mock and full-catalog replay paths.
Quote-only backtests call on_quote and do not contain depth/OI.

This is five-level SNAPSHOT data. There are no unique public trade IDs, aggressor
side labels, individual order events or a guaranteed every-trade tape. Volume-delta
or aggressor estimates must be labelled estimates and reset after feed gaps,
reconnects, cumulative-volume resets and trading-session changes. Do not equate
last_quantity with a new unique trade on every received packet.

## Verification and review focus

Final workspace gate: 173 passed, zero failed. Feature-enabled adapter gate:
64 passed, zero failed. Clippy, rustfmt and diff checks passed. One existing ignored
child fixture is exercised by its parent crash test. The saved live-data audit
verified all 28 packets in fc63b2f5-56e1-4fda-8d54-7914e8517c72, including all ten
populated depth levels and both timestamps. This was saved-live-data verification,
not a claim of a currently running recorder.

This turn removed 18 obsolete completed-simulation Redis keys; all four credential
keys, account controls, budgets and unfinished runs were preserved. See
NativeHardeningCleanup.json for the count and namespaces.

Offline full-node scenarios:

    cargo run --locked -p kite-node -- native-kite-mock config/strategy-crossover.toml
    cargo run --locked -p kite-node -- native-kite-mock-short config/strategy-crossover.toml


Tests cover long/short roundtrips, full replay, reducing exit validation, stop/target
boundaries, native cache/journal review, account contention, Redis crash/restart,
budget retention, replaced ownership, bounded retries, abort/join and SIGTERM.
Sandbox tests cover credential isolation, fixed routes and config rejection.
The original native execution/TWAP vendor patches are unchanged.

Logs: /tmp/kite-hardening-workspace-tests.log, /tmp/kite-hardening-clippy.log,
/tmp/kite-hardening-feature-tests.log and /tmp/kite-full-live-audit.log.
Review coordination.rs, dispatch.rs, ledger.rs, outage.rs, shutdown.rs, sandbox.rs,
native_node/actor.rs, signals.rs, strategy.rs, data.rs and runner.rs before enabling
any broader execution scope. Real-order activation is outside this checkpoint.

Primary contracts:
- https://kite.trade/docs/connect/v3/sandbox/
- https://kite.trade/docs/connect/v3/exceptions/
- https://kite.trade/docs/connect/v3/websocket/

# Code review guide: Rust Nautilus integration

Prepared 15 September 2026 for the next manual review.
Repository: /home/ubuntu/RustNautilasProject.
Implementation checkpoint inspected: d858e39.
This guide maps the current code; it does not certify readiness for real trading.

## 1. Current scope and review order

Real broker orders remain disabled. Supported native strategy execution is one contract, long or short, LIMIT/DAY, with one pending order. OpenBull integration has been deferred at the user's request. No webhook execution is implemented.

Read these components in order:
1. Strategy parameters and signal decisions.
2. Actor admission rules and local stop-loss/target handling.
3. Runtime backend selection and real-order disablement.
4. Command persistence, account ownership and rate limits.
5. Broker reconciliation, outage handling and shutdown.
6. Full-tick ingestion, recording and replay.
7. Tests and the two narrow upstream patches.

Paths below are repository-relative; links open the actual files from this document.

## 2. Entry points and runtime

| File | Responsibility | Review focus |
| --- | --- | --- |
| [main.rs](../apps/kite-node/src/main.rs) | Application entry point. | Trace command dispatch. |
| [cli.rs](../apps/kite-node/src/cli.rs) | General command parsing. | Distinguish native commands from earlier diagnostic commands. |
| [native_node/cli.rs](../apps/kite-node/src/native_node/cli.rs) | Dispatches native simulation, paper, backtest, mock, sandbox, status and review commands. | Explicit rejection of native-node-live; exact argument handling. |
| [native_node/runner.rs](../apps/kite-node/src/native_node/runner.rs) | Constructs native runtime, registers strategy/data/execution components and selects the backend. | Which credentials, feed and execution factory each mode receives. |
| [native_node/lifecycle.rs](../apps/kite-node/src/native_node/lifecycle.rs) | SIGINT/SIGTERM, completion and runtime deadline handling. | Shutdown reaches native clients and joins their tasks. |

The native LiveNode/Kernel/Trader, data engine, risk engine, execution engine and portfolio are Nautilus components wired by this application. They are not independently reimplemented Rust services here.

## 3. Where BUY and SELL signals originate

| File | Responsibility | Review focus |
| --- | --- | --- |
| [strategy.rs](../apps/kite-node/src/native_node/strategy.rs) | UserStrategy owns native SMA indicators and produces position-aware intents in on_quote / on_full_tick. | This is the main file to edit for your own native strategy. |
| [signals.rs](../apps/kite-node/src/native_node/signals.rs) | Defines BUY, BUY_EXIT, SELL, SELL_EXIT and local protection decisions. | Exit side, matching position and stop/target boundaries. |
| [actor.rs](../apps/kite-node/src/native_node/actor.rs) | Native strategy lifecycle, data subscriptions, quote validation, pending-order and position checks, order creation and events. | An intent must pass admission before becoming a native order. |
| [audit.rs](../apps/kite-node/src/native_node/audit.rs) | Independent native data actor used to observe delivery. | Audit quote counts alongside strategy counts. |
| [strategy-crossover.toml](../config/strategy-crossover.toml) | Fast/slow periods, freshness/spread limits, entry cap, short switch and protection distances. | Sample values are not an optimized strategy. |
| [kite-strategy/config.rs](../crates/kite-strategy/src/config.rs) | Shared strategy configuration parsing and validation. | Defaults and rejected parameter combinations. |

The current example uses 3-tick and 5-tick moving averages of quote midpoint. These are tick counts, not 3-minute/5-minute candle periods. on_full_tick currently delegates to quote logic; it does not yet calculate order-flow signals.

| Intent | Allowed starting position | Broker side | Intended ending position |
| --- | --- | --- | --- |
| BUY | Flat | BUY | Long one contract |
| BUY_EXIT | Long one contract | SELL, reducing | Flat |
| SELL | Flat, short enabled | SELL | Short one contract |
| SELL_EXIT | Short one contract | BUY, reducing | Flat |

An upward crossover opens a long from flat or exits an existing short. A downward crossover exits a long or opens a short from flat when enabled. One signal does not simultaneously exit and reverse.

Local protection uses the native average entry price and executable bid for longs / ask for shorts. Current stop distance is 30 rupees per barrel and target distance is 60. For the configured 100-barrel contract these correspond to 3,000 / 6,000 INR before fees and execution differences.

Protection is a local tick-triggered LIMIT exit. It is not a broker-held stop or guaranteed fill. Gaps, loss of data, a pending order or a resting limit can delay/prevent an exit. Freshness and valid-depth checks still apply; reducing exits may bypass the entry spread cap.

## 4. Native execution adapter and account controls

All files in this table are under crates/kite-adapter/src/execution/.

| File | Responsibility | Review focus |
| --- | --- | --- |
| [native.rs](../crates/kite-adapter/src/execution/native.rs) | Converts native orders into supported Kite commands. | LIMIT/DAY scope, quantity, identity and reducing-side validation. |
| [native_client/mod.rs](../crates/kite-adapter/src/execution/native_client/mod.rs) | Native ExecutionClient/factory, lifecycle, submit/cancel entry points, report polling and task admission. | Production factory has no dispatcher; admission is checked around asynchronous work. |
| [native_client/dispatch.rs](../crates/kite-adapter/src/execution/native_client/dispatch.rs) | Serializes broker mutations, journals ownership, reserves budget and reconciles results. | HTTP acknowledgement is not a fill; uncertain mutations are not automatically retried. |
| [native_client/ledger.rs](../crates/kite-adapter/src/execution/native_client/ledger.rs) | Redis command state, order/tag ownership and durable transitions. | Persist before network mutation, CAS transitions and WAITAOF failure handling. |
| [native_client/coordination.rs](../crates/kite-adapter/src/execution/native_client/coordination.rs) | Durable account owner, heartbeat, cooldown, budget integration and status. | No TTL takeover; only clean reconciled shutdown releases ownership. |
| [native_client/broker.rs](../crates/kite-adapter/src/execution/native_client/broker.rs) | Broker abstraction and Kite account/order/trade/position snapshots. | Stable snapshots, account identity, product and exposure. |
| [broker_events.rs](../crates/kite-adapter/src/execution/broker_events.rs) | Validates broker events and maps order/fill identity, quantities and timestamps. | Unknown/foreign state must not be accepted as this strategy's state. |
| [native_client/reports.rs](../crates/kite-adapter/src/execution/native_client/reports.rs) | Builds native account, order, fill and position reports. | Units, balances and reconciliation consistency. |
| [native_client/fees.rs](../crates/kite-adapter/src/execution/native_client/fees.rs) | Calculates charges from virtual contract notes and allocates them across fills. | Calculated versus actual charges; sandbox zero-fee estimates are explicitly limited. |
| [native_client/outage.rs](../crates/kite-adapter/src/execution/native_client/outage.rs) | Bounded retries for classified read failures. | Session rejection and rate limits differ from transient failures. |
| [native_client/shutdown.rs](../crates/kite-adapter/src/execution/native_client/shutdown.rs) | Bounded task draining, abort and join. | No successful shutdown report while work is unresolved. |
| [native_client/recovery.rs](../crates/kite-adapter/src/execution/native_client/recovery.rs) | Offline command journal review. | No broker connection, resubmission or automatic resume. |
| [transport.rs](../crates/kite-adapter/src/execution/transport.rs) | HTTP mutation transport, including separate official sandbox routing. | Production feature gate and fixed sandbox destination. |

Shared rate-limit implementation: [policy.rs](../crates/kite-execution/src/rate_limit/policy.rs) and [store.rs](../crates/kite-execution/src/rate_limit/store.rs). Native account budgets are 5 commands/second, 100/minute and 1,000/rolling day. Failed attempts and cancels consume budget. Coordination covers participating application processes, not unrelated scripts or manual broker activity.

Known documentation nit for review: dispatch.rs has an older module comment saying only the mock factory constructs its service. The official SandboxFactory also attaches a dispatcher; inspect constructors and backend wiring as the source of truth.

## 5. Execution modes and configuration

| File / component | Actual role and current status |
| --- | --- |
| [native_client/mock.rs](../crates/kite-adapter/src/execution/native_client/mock.rs) | Deterministic in-process broker fixture; tests the adapter path without external broker access. |
| Nautilus SandboxExecutionClient | Simulated execution used by native simulation/paper runtime; this is distinct from Zerodha's hosted sandbox. |
| [native_client/sandbox.rs](../crates/kite-adapter/src/execution/native_client/sandbox.rs) | Official Kite sandbox preflight, verified-account setup and execution factory. |
| [kite-sandbox.toml](../config/kite-sandbox.toml) | Official sandbox settings; expected_user_id remains a deliberate placeholder until authentication/account verification succeeds. |
| [native_client/custom_sandbox.rs](../crates/kite-adapter/src/execution/native_client/custom_sandbox.rs) | Credential-free GET compatibility probe for the supplied Nordible server; cannot execute orders. |
| [kite-custom-sandbox.toml](../config/kite-custom-sandbox.toml) | Address for that compatibility probe only. |
| [crudeoil-september.toml](../config/crudeoil-september.toml) | Contract selection for CRUDEOIL26SEPFUT.MCX; review expiry and token before any later session. |
| OpenBull/OpenAlgo-style sandbox | Proposed separately, then deferred; no adapter or validated integration exists in this checkpoint. |

There is no enabled real-execution configuration to switch on for this review. Backend selection is in runner.rs; the production native factory stays read-only even with the older live-orders feature enabled.

Last official sandbox checks returned HTTP 403. Credentials existing in Redis do not prove authentication works. The supplied Nordible server returned static/incompatible responses. Neither result is evidence of successful external sandbox fills.

## 6. Market data and order-flow inputs

| File | Responsibility | Review focus |
| --- | --- | --- |
| [websocket/models.rs](../crates/kite-adapter/src/websocket/models.rs) | Tick/depth field models. | Price units, optional fields and timestamp meanings. |
| [websocket/parser.rs](../crates/kite-adapter/src/websocket/parser.rs) | Binary Kite packet decoding. | Full-packet size and field offsets. |
| [websocket/subscription.rs](../crates/kite-adapter/src/websocket/subscription.rs) | Instrument subscription and feed mode. | Full mode and correct instrument token. |
| [websocket/transport.rs](../crates/kite-adapter/src/websocket/transport.rs) | Socket connection and transport. | Endpoint separation and shutdown. |
| [websocket/supervisor.rs](../crates/kite-adapter/src/websocket/supervisor.rs) | Feed supervision, freshness watchdog and bounded reconnects. | Heartbeats alone must not count as usable data. |
| [data/full_tick.rs](../crates/kite-adapter/src/data/full_tick.rs) | Complete Kite snapshot plus native quote payload. | Raw depth/OI/volume retained beyond top-of-book quotes. |
| [mapping/quotes.rs](../crates/kite-adapter/src/mapping/quotes.rs) | Native quote conversion. | Top-of-book price and quantity conversion. |
| [native_node/data.rs](../apps/kite-node/src/native_node/data.rs) | Native DataClient, delivery into the node and capture. | Task lifetime, full-packet delivery and capture bounds. |
| [native_node/status.rs](../apps/kite-node/src/native_node/status.rs) | Custom feed status data. | Strategy reaction to unavailable or stale data. |
| [native_node/full_codec.rs](../apps/kite-node/src/native_node/full_codec.rs) | Arrow codec preserving complete packet JSON and indexed timestamps. | Round-trip payload equality. |
| [native_node/catalog.rs](../apps/kite-node/src/native_node/catalog.rs) | Quote/full-packet catalog writing, reading and audit. | Historical data persistence is separate from Redis application state. |

Available full packets include last price/quantity, cumulative volume, average price, total buy/sell quantities, OHLC, OI and OI day range, trade/exchange timestamps, and five bid/ask levels with price, quantity and order count. Capture also tracks reception time and feed continuity information.

Saved live capture fc63b2f5-56e1-4fda-8d54-7914e8517c72 was audited: all 28 packets had full fields, both source timestamps and five populated levels on each side. This proves that saved sample, not that recording is currently running.

Five-level snapshots do not provide unique trade IDs, reliable aggressor side, every exchange trade, or order-by-order add/cancel deltas. An order-flow strategy must account for gaps and volume/session resets and identify inferred measurements.

## 7. Redis persistence, credentials and recovery

| File / directory | Responsibility |
| --- | --- |
| [native_node/persistence.rs](../apps/kite-node/src/native_node/persistence.rs) | Native cache and Redis configuration. |
| [native_node/redis_cache.rs](../apps/kite-node/src/native_node/redis_cache.rs) | Installs the native Nautilus Redis cache adapter. |
| [native_node/recovery.rs](../apps/kite-node/src/native_node/recovery.rs) | Read-only reconstruction of native cached state for a namespace. |
| [credentials/redis.rs](../crates/kite-adapter/src/credentials/redis.rs) | Separate production/sandbox credential lookup without sandbox fallback to production. |
| [http/authenticated.rs](../crates/kite-adapter/src/http/authenticated.rs) | Authenticated reads, endpoint routing and classified HTTP errors. |
| [kite-journal/src](../crates/kite-journal/src) | Shared Redis connection, journal state and durable application-state operations. |

Important native key families:
- Account coordination: susanta:nautilus:native-kite:account:{ACCOUNT}.
- Command journal: susanta:nautilus:native-kite:commands:{NAMESPACE}.
- Rate budget: susanta:nautilus:sim:order-budget:{native-account-ACCOUNT}.
- Native cache: instance-specific UUID namespaces.

Never interpret an old heartbeat as permission to delete an owner key. Recovery requires proving the old process stopped and reconciling broker orders, trades and exposure against retained state. Review commands do not unlock accounts or resume trading. Keep budgets, credentials and unresolved journals. No SQLite is introduced for application/order state.

## 8. Backtesting, auxiliary code and upstream patches

| File / directory | Responsibility |
| --- | --- |
| [native_node/backtest.rs](../apps/kite-node/src/native_node/backtest.rs) | Native BacktestNode/BacktestEngine runs and result reporting, including saved full-packet replay. |
| [native_node/components.rs](../apps/kite-node/src/native_node/components.rs) | Synthetic OrderEmulator and TWAP integration scenarios; not the default strategy's execution policy. |
| [kite-recorder/src](../crates/kite-recorder/src) | Earlier recorder records/writer/replay utilities. |
| [kite-strategy/crossover.rs](../crates/kite-strategy/src/crossover.rs) | Earlier crossover implementation; the active native strategy is native_node/strategy.rs. |
| [paper_flow](../apps/kite-node/src/paper_flow) and [kite-paper/src](../crates/kite-paper/src) | Earlier paper orchestration, worker, validation and outbox paths. |
| [kite-execution/src](../crates/kite-execution/src) | Shared/earlier execution coordination, translation, management, reports and rate limiting. |
| [runtime](../apps/kite-node/src/runtime) | Earlier capture/replay runtime commands. |
| [reconciliation](../crates/kite-adapter/src/reconciliation) | Broker snapshot/check utilities used by diagnostic/preflight paths. |
| [instruments](../crates/kite-adapter/src/instruments) | Contract definitions, instrument master parsing and resolution. |
| [Cargo.toml](../Cargo.toml) / [Cargo.lock](../Cargo.lock) | Workspace members, pinned dependencies and reproducible resolution. |
| [vendor/README.md](../vendor/README.md) | Upstream patch provenance and rationale. |
| [nautilus-execution.patch](../vendor/nautilus-execution.patch) | Narrow marketable-LIMIT event/cache ordering fix. |
| [nautilus-trading.patch](../vendor/nautilus-trading.patch) | TWAP start hook forwarding fix. |

Quote-only backtests cannot validate an order-flow strategy that depends on full depth/OI. Use a complete packet catalog for that review. A 15-tick fixture verifies integration mechanics, not profitability or realistic execution quality.

## 9. Verification evidence and reviewer commands

Latest recorded implementation gates: 176 workspace tests passed, Clippy with warnings denied, rustfmt and diff checks passed. Earlier hardening checkpoint: 64 adapter tests with the legacy live-orders feature passed. Upstream patch checkpoint: 1,152 execution tests and 39 TWAP tests passed, with one existing ignored upstream test. These are prior results, not tests rerun for this documentation-only change.

Test entry points:
- [native_components.rs](../apps/kite-node/tests/native_components.rs): native integration scenarios.
- [native_client tests](../crates/kite-adapter/src/execution/native_client): dispatch, coordination, outage, fee, shutdown and sandbox probe tests.
- [adapter integration tests](../crates/kite-adapter/tests): credentials, packets, mappings and preflight.
- [journal durability tests](../crates/kite-journal/tests/durability.rs): persistence and crash behavior.

Run from /home/ubuntu/RustNautilasProject:

```bash
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Optional synthetic execution checks (write test state/catalog evidence; no real orders):

```bash
cargo run --locked -p kite-node -- native-backtest config/strategy-crossover.toml
cargo run --locked -p kite-node -- native-kite-mock config/strategy-crossover.toml
cargo run --locked -p kite-node -- native-kite-mock-short config/strategy-crossover.toml
```

Read-only operational review (replace NAMESPACE with the actual run UUID):

```bash
cargo run --locked -p kite-node -- native-kite-status MOCK
cargo run --locked -p kite-node -- native-kite-review NAMESPACE
cargo run --locked -p kite-node -- native-recover NAMESPACE
```

Recorded gate logs: /tmp/kite-custom-workspace-tests.log, /tmp/kite-custom-clippy.log, /tmp/kite-hardening-feature-tests.log and /tmp/kite-full-live-audit.log.

## 10. Manual review checklist

- [ ] Confirm intended strategy decisions in on_quote / on_full_tick.
- [ ] Check all four intents against current position and reduce-only rules.
- [ ] Check tick-period assumptions, price units, multiplier and contract expiry.
- [ ] Confirm stop/target behavior with stale data, pending orders and unfilled limits.
- [ ] Trace native risk checks and application limits before broker dispatch.
- [ ] Confirm real factory cannot dispatch and credentials never cross environments.
- [ ] Check durable journal writes precede requests and mutation uncertainty cannot cause duplicate orders.
- [ ] Check account owner fencing and budget retention across failure/restart.
- [ ] Check foreign orders/positions, partial fills and inconsistent snapshots fail safely.
- [ ] Check shutdown retains review-required state unless reconciled flat and healthy.
- [ ] Verify full-packet replay for any depth-dependent strategy changes.
- [ ] Review both vendor patches and relevant tests.
- [ ] Leave external sandbox validation and real trading explicitly unapproved/pending.

Operational monitoring currently means emitted status/events, account heartbeat/recovery state and retained logs. This is not a deployed external alerting dashboard or unattended incident-response service.

Related detail: [NativeHardening.md](NativeHardening.md), [NativeIntegrationHandoff.md](NativeIntegrationHandoff.md), [NativeKiteExecution.md](NativeKiteExecution.md), [CustomSandbox.md](CustomSandbox.md).

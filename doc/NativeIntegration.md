# Native Nautilus integration: architecture and verification

Latest Kite-only checkpoint: [NativeKiteExecution.md](NativeKiteExecution.md).
Native Kite dispatch, event mapping and mass reconciliation are tested with a
deterministic broker fixture. Real API execution remains disabled.

Verified on 15 September 2026 in `/home/ubuntu/RustNautilasProject` on
`ip-172-31-36-59`, with Rust 1.98.0 and published Nautilus crates pinned to 0.63.0.
The integration is **not complete**: native live Kite execution remains open.
The native startup and immediate LIMIT event-order issues were subsequently fixed;
see [NativeEventCompatibility.md](NativeEventCompatibility.md). No hardening phase was started.
Real orders remain disabled. See Git history for the integration commit; the
verification results below were recorded before that commit.

## Native component map

All paths below are relative to `apps/kite-node/src/native_node/`.

| Component | Integration / verification | Module |
| --- | --- | --- |
| LiveNode, Kernel, Trader, live clock | Own the live-data and synthetic-paper lifecycle | runner.rs |
| Strategy | Native Strategy lifecycle, callbacks, native order API | actor.rs |
| User signals and indicators | Native SMA crossover; editable full-packet and quote callbacks | strategy.rs |
| Actor | Independent audit quote subscription | audit.rs |
| DataClient and factory | Kite full packets plus derived QuoteTicks and feed status on native data channel | data.rs, status.rs |
| DataEngine, MessageBus | Native node routing to strategy/audit and matching | runner.rs, backtest.rs |
| RiskEngine | Native order validation and notional limit; application position/feed checks | runner.rs, actor.rs |
| ExecutionEngine | Native strategy orders and lifecycle events with simulated execution | runner.rs, backtest.rs |
| Matching engine | Native Sandbox for LiveNode; native simulated venue for backtests | runner.rs, components.rs |
| Portfolio and accounts | Native simulated INR margin accounts, fills and positions; not actual broker balances | runner.rs, backtest.rs |
| Cache / Redis | Nautilus Redis backing with unique trader/instance prefix | persistence.rs, redis_cache.rs |
| Redis reconstruction | Native load_all for instruments, orders, accounts and positions, without resubmission | recovery.rs |
| Data catalog | Native ParquetDataCatalog, QuoteTicks and complete custom KiteFullTick packets | catalog.rs, full_codec.rs |
| BacktestNode / BacktestEngine | Quote catalog streaming; full packets explicitly loaded into the node-owned engine for one-shot replay | backtest.rs |
| OrderEmulator | Native stop trigger and closing trade exercised | components.rs |
| TWAP | Registered on node; child-order execution exercised in native backtest | runner.rs, backtest.rs, components.rs |
| Native Kite ExecutionClient | **Missing**: existing guarded HTTP service is not wired to the native node | existing adapter execution modules are separate |
| Controller / tearsheets | Not part of the supported pure-Rust registration/report surface | upstream capability limitation |

The [official Rust capability matrix](https://nautilustrader.io/docs/latest/concepts/rust/)
marks Controller registration and tearsheets as Python-only. Controller internals
exist in Rust, but the supported importable-controller registration path is Python-only.
Backtest results already include native statistics; these are not HTML tearsheets.

## Event and storage flow

Kite WebSocket supervisor -> native DataClient -> LiveNode data channel ->
DataEngine/MessageBus -> native QuoteTick and KiteFullTick subscriptions.
The strategy consumes the full packet and native net position, then submits native
orders through RiskEngine -> ExecutionEngine -> Sandbox matching. Native order
and fill events update cache, portfolio and strategy callbacks. AuditActor counts
quotes independently of strategy quality rejection.

Order/account/position and saved application state use Redis. Historical market
data uses Parquet. Native Redis prefixes include `trader-SUSANTA-001` and a unique
instance UUID. No existing database is flushed. `KITE_REDIS_URL` must refer to
database 0 with AOF enabled; credentials retain the existing Redis mechanism.
Native Redis writes are asynchronous. The old journal's per-command WAITAOF
barrier is not a guarantee of this new runtime. Automatic resume is disabled.

`redis_cache.rs` now uses the native Redis adapter without event filtering.
The local matching-engine patch fixes immediate LIMIT ordering at its source;
reconstruction tests assert the exact persisted lifecycle sequence.

On orderly completion, full packets are written through the registered Arrow
codec and read back through the native catalog. Exact equality includes the raw
packet, depth levels, OHLC, volume, open interest, metadata and native quote.
DataFusion can return Utf8View payloads; the decoder normalizes them to Utf8 and
checks event/init timestamps. The initial readback test exposed this issue and
passed after the fix. Successful node output now explicitly reports
`full_catalog_roundtrip_verified` and `captured_full_packets`.

A catalog containing KiteFullTick data selects full-packet BacktestNode replay.
The Rust 0.63.0 BacktestDataConfig enum has no Custom variant, so the application
loads full packets explicitly into the built node's native BacktestEngine.
Derived quotes precede their full packets; generation changes initialize feed
status and reset strategy warmup. Quote-only catalogs retain the streaming path.
This is packet replay, not an exact reconstruction of every transport-gap event
or the wall-clock timing of live execution. Quote-only catalogs cannot test
signals that require full-packet fields.

## Strategy edit point

Edit `apps/kite-node/src/native_node/strategy.rs`.

- `UserStrategy::on_full_tick`: full live/synthetic packets and full-catalog replay.
- `UserStrategy::on_quote`: shared reference crossover and quote-only replay.
- `new` / `reset`: indicator initialization and warmup state.

The default full-tick method delegates to the quote policy. Keep strategy-specific
signals in this file. The actor owns runtime lifecycle, order construction,
quality checks and persistence separately. The current runtime enforces one long
contract, one pending order, entry limits and LIMIT/DAY orders. A strategy requiring
shorts, different sizing or new order types also needs a runtime contract change;
editing signals alone does not remove those restrictions.

Parameters: `config/strategy-crossover.toml`. Instrument selection:
`config/crudeoil-september.toml`. The contract is resolved before entering the
native event loop; token and contract metadata are not selected in strategy code.

## Commands and execution gate

Run from `/home/ubuntu/RustNautilasProject`:

```bash
cargo run --locked -p kite-node -- native-node-sim config/strategy-crossover.toml
cargo run --locked -p kite-node -- native-node-paper config/crudeoil-september.toml config/strategy-crossover.toml --seconds 30
cargo run --locked -p kite-node -- native-backtest config/strategy-crossover.toml
cargo run --locked -p kite-node -- native-backtest config/strategy-crossover.toml data/native-catalog/INSTANCE_UUID
cargo run --locked -p kite-node -- native-emulator-sim
cargo run --locked -p kite-node -- native-twap-sim
cargo run --locked -p kite-node -- native-recover INSTANCE_UUID
```

There is currently **no usable native live-order enable flag**.
`native_node/runner.rs` configures `Environment::Sandbox` and registers only
`SandboxExecutionClientFactory`. `native_node/cli.rs` rejects `native-node-live`.
The adapter's `live-orders` Cargo feature is disabled and would be insufficient
by itself: no native Kite order client is wired to this node. Changing a printed
`live_orders_enabled` field does not change execution behavior.

Paper shutdown cancels pending orders; it does not invent a closing market fill
for an open simulated position. Such a position stays visible in Redis recovery
with `requires_review=true`. Recovery never resumes trading.

## Verification evidence

```bash
cargo test --locked --workspace -j 2
cargo clippy --locked --workspace --all-targets -j 2 -- -D warnings
cargo fmt --all -- --check
git diff --check
cargo tree --locked -e features -i kite-adapter
```

Workspace: **133 passed, 0 failed**. The single ignored top-level test is
`abrupt_exit_child`, deliberately launched by its passing parent crash-recovery
test with `--ignored --exact`; it is not an unexecuted functional test.
Clippy, formatting and diff checks passed. The feature tree shows only the
adapter's default feature, with `live-orders` absent.

| Scenario | Observed result |
| --- | --- |
| Synthetic BacktestNode | 15 strategy ticks, 2 signals, 2 fills, flat |
| Synthetic LiveNode | 15 strategy ticks, 2 signals, 2 fills, flat; 15 complete packets saved/read back |
| Full-packet replay of synthetic capture | 15 packets/ticks, 2 signals, 2 fills, flat |
| Native Redis reconstruction | 2 orders, 1 account, no open orders/position, zero resubmissions |
| OrderEmulator | 1 emulated trigger, 2 fills, flat |
| TWAP | Spawned child orders, 3 fills including exit, flat |
| Live command rejection | Nonzero exit; paper execution remains enforced |
| Full catalog codec | Complete packet equality and corresponding quote equality verified |

Two bounded live Kite data checks ran with Sandbox only:

1. `cbf0a145-c68c-4b4b-a6b0-c8562689d60c`: 32 audit quotes, 16 accepted
   strategy ticks, 16 spread rejections, 1 signal and 1 simulated fill. It ended
   with one simulated long contract. Native Redis recovery reproduced that
   position with `requires_review=true` and zero resubmissions.
2. `fc63b2f5-56e1-4fda-8d54-7914e8517c72`: after the codec fix, 28 audit
   quotes/complete packets, 16 accepted ticks, 12 spread rejections, no signals
   or fills, flat. All 28 packets passed native catalog readback. Backtest replay
   reproduced 28 audit quotes and 16 accepted ticks. Redis reconstruction found
   one account, no orders/positions, and zero resubmissions.

Both reported `live_orders_enabled=false` and `broker_orders_accessed=false`.
The first run's simulated order exposed the known Sandbox InvalidStateTrigger
warnings. The second run generated no orders and therefore does not demonstrate
that those warnings have been resolved.

Raw logs are in `/tmp/kite-native-final-tests.log`,
`/tmp/kite-native-final-clippy.log`, `/tmp/kite-native-paper-check.log`,
`/tmp/kite-native-paper-final.log`, `/tmp/kite-native-live-replay.log`,
`/tmp/kite-native-live-recovery.log`, and
`/tmp/kite-native-live-position-recovery.log` on the remote host.

## Native Kite adapter checkpoint

NativeKiteExecution.md documents the integrated LIMIT/DAY adapter, Redis-owned
mock dispatch, native delayed-update polling, calculated-fee reports and mass
reconciliation. native-kite-mock registers the adapter through LiveNode and verifies
two fills, flat position and both native-cache and command-journal reconstruction.
The real client remains read-only. No real orders or hardening have been performed.

Native Sandbox immediate LIMIT event ordering and TWAP startup hooks are fixed by
pinned local patches. NativeEventCompatibility.md records upstream test results and
the existing ignored upstream test. Controller registration/tearsheets remain outside
the supported pure-Rust capability matrix; no placeholders are claimed implemented.

The next phase is production hardening within the documented supported scope.
Keep all real-order gates disabled. Broader order types need their own integration
coverage before strategy exposure. Real-order verification requires authorization.
Older temporary verification logs were removed at the user's request; retained
latest logs are listed in NativeKiteExecution.md and NativeIntegrationHandoff.md.

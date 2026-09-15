# Native Nautilus integration handoff

Updated 15 September 2026. Project: /home/ubuntu/RustNautilasProject.
Branch main; use git log -1 for the current checkpoint commit.

## Scope and safety

The supported Kite-native LIMIT/DAY integration is complete and fixture-tested.
Real broker orders remain disabled. No webhook integration is included.
Production hardening is the next phase; this checkpoint is not production-ready.
Redis owns order/application state; historical packets use the native catalog.
User authorized committing/pushing this checkpoint and cleaning obsolete logs/keys.

Read NativeKiteExecution.md for the adapter, supported scope and limitations.
Read NativeIntegration.md for the broader component map, and
NativeEventCompatibility.md plus vendor/README.md for upstream patch provenance.

## Integrated components

Nautilus 0.63.0 LiveNode, Kernel, Trader, DataClient, Strategy, AuditActor,
risk/execution engines, portfolio/accounts, native Redis cache, native catalog,
BacktestNode/BacktestEngine, matching engine, OrderEmulator, SMA and TWAP.
Two minimal vendored patches fix immediate LIMIT event ordering and TWAP startup.
Full Kite packets and quotes survive catalog roundtrip and native backtest replay.

The native Kite ExecutionClient provides translation, validated broker events,
Redis-owned submit/cancel dispatch, lost-ack correlation, delayed-update polling,
and owned order/fill/position/mass reports. Actual Kite API bindings include
read-only account/position/order/trade reads and virtual-contract-note charges.
Tests use local HTTP fixtures and a deterministic broker, with no real requests.
The real factory remains read-only; only MockFactory attaches the dispatcher.
Calculated fees are allocations, not settled per-fill broker commissions.
No automatic restart/resume or command resubmission is implemented.

## User entry points

Strategy: apps/kite-node/src/native_node/strategy.rs, UserStrategy.on_full_tick
and on_quote. Parameters: config/strategy-crossover.toml.
Instrument: config/crudeoil-september.toml, CRUDEOIL26SEPFUT.MCX.
Scope: one long contract, one pending order, LIMIT/DAY, bounded entries.

Execution selection: apps/kite-node/src/native_node/runner.rs.
Native simulation/paper uses Environment::Sandbox and SandboxExecutionClientFactory.
Native-kite-mock uses the new MockFactory. Real-client configuration is in
crates/kite-adapter/src/execution/native_client/mod.rs.
Native-node-live is rejected by native_node/cli.rs. There is no usable real-order
configuration file or enable flag. The legacy live-orders feature cannot activate
the native real client. Real-order activation requires separate authorization.

    cargo run --locked -p kite-node -- native-backtest config/strategy-crossover.toml
    cargo run --locked -p kite-node -- native-kite-mock config/strategy-crossover.toml

Both verified synthetic runs produce 15 ticks, two signals, two fills and flat
positions. The mock tests adapter plumbing; backtest uses native matching.
Latest standalone mock namespace: 89493f0b-7182-4ee6-b37d-4c6c92ab798c.

## Verification

- cargo test --locked --workspace -j 2: 159 passed, zero failed.
- One ignored child-process fixture is exercised by its passing parent crash test.
- Feature-enabled native-client tests: 16 passed, zero failed.
- Upstream patched execution: 1152 passed; one existing ignored trailing-stop test.
- Upstream patched TWAP: 39 passed.
- Workspace Clippy all-targets with -D warnings, rustfmt and diff checks passed.
- Native event histories assert Initialized, Submitted, Accepted, Filled exactly.
- Full packet catalog replay, emulator, TWAP, recovery and live rejection passed.

Logs retained in /tmp: kite-native-only-tests.log, kite-native-only-clippy.log,
kite-native-client-feature-tests.log, kite-upstream-execution-tests.log,
kite-upstream-twap-tests.log, kite-native-patched-backtest.log,
kite-native-patched-sim.log, kite-native-kite-mock.log, kite-native-paper-final.log.

## Redis and log cleanup

Removed 104 obsolete Redis keys: seven flat historical native namespaces and eight
empty simulation journals. Preserved all other 170 keys, including credentials,
latest verification runs, unfinished simulated runs and order budgets.
No credential values were read. No database flush or Redis configuration changes.
Removed 91 obsolete project log files from /tmp across this continuation.
Exact cleanup counts/namespaces: NativeCleanup.json. Catalogs remain available.

The historical live-data simulation cbf0a145-c68c-4b4b-a6b0-c8562689d60c has a
simulated long requiring review; preserve its evidence. This is not a real position.
Latest bounded paper run fc63b2f5-56e1-4fda-8d54-7914e8517c72 was flat, with 28 full
packets roundtripped. Both had real orders disabled. Native cache writes remain
asynchronous; the separate new command journal adds explicit WAITAOF barriers.

## Next phase

Harden account-wide command fencing/rate budgets, operational reconciliation,
restart review, bounded read retries/health handling and shutdown during outages.
Keep real orders disabled throughout. Unsupported order types fail explicitly.

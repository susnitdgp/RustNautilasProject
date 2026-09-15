# Native Nautilus integration handoff

Updated 15 September 2026 after continuation and verification.
Project: /home/ubuntu/RustNautilasProject on ip-172-31-36-59.
Remote Desktop Commander device: 90c36da5-2611-4429-93f0-fbde579cdcb9.
Branch main. This checkpoint follows de56cfd and is included in the native
integration commit. Use git log -1 for the current commit identifier.

## User instructions

Integrate applicable native Nautilus components before hardening.
Run tests/checks autonomously. Keep real orders disabled.
Use Redis for order and application state; historical data may use Parquet.
Keep components in separate modules. Do not commit/push until requested.
Finally identify the strategy edit point and live execution configuration honestly.

## Current implementation and verification

Read doc/NativeIntegration.md first for the full current component map and evidence.
Nautilus crates are pinned =0.63.0; Rust 1.98.0. No upstream source modifications.
Native modules are in apps/kite-node/src/native_node.
LiveNode/Kernel/Trader owns lifecycle; native DataClient routes full Kite packets
plus quotes/status; native Strategy/AuditActor, risk/execution engines, Sandbox,
portfolio/accounts, Redis cache, BacktestNode, OrderEmulator and TWAP are exercised.

Continuation fixed full-codec Arrow decoding: DataFusion returns Utf8View where
the initial decoder expected StringArray. Normalize payload to Utf8 and validate
both native timestamps. Full packet and quote equality now pass a real native
catalog write/read test. Node writes verify all captured packets by native readback.

BacktestNode full-packet replay is implemented: read catalog custom packets into
the built node's native BacktestEngine and run one-shot with quotes preceding their
full packets. Feed generation changes initialize status. Quote-only catalogs retain
streaming data configuration. This is not exact replay of every transport-gap event.

User edit point: apps/kite-node/src/native_node/strategy.rs.
UserStrategy::on_full_tick receives complete data; default delegates to on_quote.
Quote-only backtests use on_quote. Full catalog replay uses on_full_tick.
Runtime still restricts one long contract, one pending order, LIMIT/DAY and entry cap.
Parameters: config/strategy-crossover.toml. Instrument: config/crudeoil-september.toml.
Native live configuration is in native_node/runner.rs: Environment::Sandbox and
SandboxExecutionClientFactory only. native_node/cli.rs rejects native-node-live.
There is NO usable native live-order enable flag. The adapter live-orders feature
is disabled and alone would not connect native execution to Kite.

## Verified results

cargo test --locked --workspace -j 2: 133 passed, 0 failed, one top-level ignored
abrupt_exit_child fixture that is exercised by its passing parent crash test.
cargo clippy --locked --workspace --all-targets -j 2 -- -D warnings: passed.
cargo fmt --all -- --check: passed. git diff --check: passed.
cargo tree --locked -e features -i kite-adapter: default only; live-orders absent.

Synthetic LiveNode and BacktestNode: 15 ticks, 2 signals, 2 fills, flat.
Full synthetic capture/replay: 15 complete packets, 2 signals, 2 fills, flat.
Native Redis reconstruction: orders/accounts/positions loaded, no resubmissions.
OrderEmulator: 1 trigger, 2 fills, flat. TWAP: child orders, 3 fills, flat.

Bounded native-node-paper LIVE DATA runs used simulated execution only:
- cbf0a145-c68c-4b4b-a6b0-c8562689d60c: 32 audit quotes, 16 accepted ticks,
  1 simulated fill, one SIMULATED long remaining. Redis recovery reproduced it
  with requires_review=true, zero resubmissions. Do not treat this as a real position.
- fc63b2f5-56e1-4fda-8d54-7914e8517c72: 28 complete packets/quotes,
  16 accepted ticks, no signals/fills, flat. All 28 full packets read back exactly.
  Full BacktestNode replay reproduced 28 audit quotes and 16 accepted ticks.
Both reported live_orders_enabled=false and broker_orders_accessed=false.
Native persistence is asynchronous, without old journal per-command WAITAOF barriers.
No automatic resume. Shutdown cancels pending orders but does not invent flattening fills.

## Logs

/tmp/kite-native-final-tests.log
/tmp/kite-native-final-clippy.log
/tmp/kite-native-paper-check.log
/tmp/kite-native-paper-final.log
/tmp/kite-native-live-replay.log
/tmp/kite-native-live-recovery.log
/tmp/kite-native-live-position-recovery.log

## Remaining integration work (NOT COMPLETE)

1. Implement/register native Kite ExecutionClient/factory with broker command,
   order/fill event, native account/position report and reconciliation integration.
   Existing guarded HTTP services and simulated journal reports are not this client.
   Keep actual submissions disabled and validate with fixtures/mock transport first.
2. Resolve Sandbox 0.63.0 queued event compatibility. Immediate LIMIT matching applies
   Accepted locally and refreshes cached state before queued Submitted/Accepted events
   reach ExecutionEngine, causing InvalidStateTrigger warnings. Existing Redis adapter
   suppresses exact duplicate persistence notifications but does not fix event dispatch.
   Investigated a public-API wrapper restoring order snapshots: unsuitable because
   Cache::replace_order also refreshes Redis persistence and can corrupt event chronology.
   No wrapper or upstream patch was applied; warnings remain visible.
3. Controller registration and tearsheets are Python-only in the supported official
   capability matrix: https://nautilustrader.io/docs/latest/concepts/rust/ . Do not
   present placeholders as pure-Rust native integration.
4. Only after native integration gaps are closed proceed to hardening. No hardening
   phase or real-order verification was performed in this continuation.

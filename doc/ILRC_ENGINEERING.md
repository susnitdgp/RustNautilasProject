# ILRC Combined: Nautilus/Kite Engineering Design

**Implementation:** Rust 1.98.1, NautilusTrader Rust 0.64.0, Tokio, Kite Connect, Redis-backed native execution. This document describes the implemented components and tested integration path.

## Component topology

```text
Kite master / historical OHLCV -> kite-adapter::http / instruments
                                    |
                      native_node::live_data (STBARS)
                                    |
                      Nautilus DataClientFactory
                                    |
                     nautilus_live::LiveNodeBuilder
                                    |
                       IlrcActor (DataActor/Strategy)
                       |      +-- Setup A and B signals
                       |      +-- single active position
                       |      +-- stop/target/+1R management
                       v
                      Nautilus OrderFactory / OrderAny
                                    |
                       ExecutionClient / native Dispatcher
                       |      +-- RedisStore::coordinated
                       |      +-- account ownership lease
                       |      +-- broker snapshot reconciliation
                       v
                  Kite native MockFactory (tested)
                  Kite production Factory (shared infrastructure;
                  NOT registered in ILRC commands)
```

## Nautilus components actually used

| Component / crate | Purpose in project |
|---|---|
| `nautilus-core` | `UUID4` per run and high-precision timestamps |
| `nautilus-model` | `FuturesContract`, `InstrumentId`, `BarType`, `Bar`, `OrderAny`, `OrderSide`, `Quantity`, `Price`, order events and identifiers |
| `nautilus-common` | `DataActor`, `ExecutionClient`, `OrderFactory`, execution-event channels, cache/clock integration, timers |
| `nautilus-trading` | `StrategyCore`, `StrategyConfig`, `Strategy` callbacks and `nautilus_strategy!` |
| `nautilus-live` | `LiveNodeConfig`, `LiveNodeBuilder`, hosted `LiveNode` lifecycle; data and native mock execution clients |
| `nautilus-persistence` / `nautilus-infrastructure` | Native state/cache storage and Redis-backed cache factory |
| `nautilus-sandbox` | Existing alternative simulated execution factory; the ILRC native mock chooses Kite's native mock factory instead |

These components are not interchangeable: an HTTP Kite acknowledgement is not a Nautilus accepted/fill event. Native order events are emitted only after broker snapshot reconciliation.

## ILRC modules

`ilrc_backtest.rs` and `ilrc_continuation_backtest.rs` calculate strategy entries and completed trades; `ilrc_causal_audit.rs` checks their historical prefix stability. `ilrc_live_actor.rs` wraps those signals in a Nautilus `Strategy` that processes confirmed three-minute bars and submits native orders, enforces one-position arbitration, tracks entry/stop client order IDs, requests reducing protective SL-M after fills, requests modifications at +1R, and cancels the stop before target/EOD exits. Its time-event guard also requests flattening at shutdown and rejects clean stop while exposure/order state remains unresolved.

`ilrc_native_runner.rs` provides two **non-live** hosted node commands: read-only Kite market data with native mock execution, and a replay of an October 7 fixture. It uses `live_data::Factory` plus `data::Factory` for market data, the Redis cache factory, and `kite_adapter::execution::native_client::mock::MockFactory`. Mock accounts have unique IDs per run, so failure records cannot be silently reused by another run.

`ilrc_shadow.rs` is deliberately separate: it recomputes completed historical outcomes and dashboard metrics with no execution client. The selected strategy JSON validates 3-minute CRUDEOIL and broker-order gates.

## Broker transport, ownership and recovery

`crates/kite-adapter/src/execution/native.rs` maps Nautilus market and reducing stop-market orders to Kite `ProtectedMarket` and `ProtectiveStopMarket` commands. `request.rs` validates Kite command forms; `transport.rs` is the real HTTP transport, requires the optional live-orders Cargo feature, and does not automatically retry unknown mutations. The native dispatcher owns client-order-to-broker-tag association, reserves Redis before mutations, processes broker acknowledgements separately from observed fills, matches order/trade IDs, and verifies positions. Reducing SL-M updates use `ModifyProtectiveStop`, persist a management record, and emit Nautilus `OrderUpdated` only after the changed trigger is confirmed in a broker snapshot.

`native_client/ledger.rs` provides the Redis order journal and account coordination. `native_client/coordination.rs` rejects dirty restarts, existing owners, unresolved orders and unexpected positions. `native_client/recovery.rs` provides manual review. Additional `ilrc_redis_state.rs` contains an atomic CAS state journal for the staged orchestrator; it is **not yet the active native actor's journal**. The active native mock node uses the pre-existing Redis-backed dispatcher journal.

## Tested scenarios

- Workspace Rust formatting/Clippy/tests and Kite command translation.
- Incremental October 7 entries, Setup A/B priority, next-open replay and adverse slippage.
- Nautilus `LiveNode` startup/shutdown against a real Kite historical feed and native mock client.
- Historical fixture: Setup B entry, native mock fill, broker-observed SL-M acceptance and clean shutdown reconciliation.
- Dispatcher persistence before sending, duplicate attempts, lost acknowledgements, Redis ownership and stop-modification broker snapshot confirmation.
- Failure cases, including a simulated invalid fill-to-stop risk that leaves Redis `ReviewRequired` and blocks the next restart. That record is preserved, not erased.

## Running safely

The only executable ILRC command paths in this integration are `native-ilrc-shadow`, `native-ilrc-nautilus-mock`, and `native-ilrc-nautilus-fixture`. The last two can perform **mock order mutations** against a native in-memory/mock Kite broker and Redis state. They never send real Kite order mutations. The original production profile and local broker profile must both retain `live_orders_enabled=false`. **Do not interpret successful mock assertions as authorization for real-money execution.**

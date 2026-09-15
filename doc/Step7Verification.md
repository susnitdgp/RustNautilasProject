# Step 7: native Nautilus paper execution

Run on the development server:

```bash
cd /home/ubuntu/RustNautilasProject
cargo run --locked -p kite-node -- nautilus-paper-sim config/strategy-crossover.toml
```

Wait for the final JSON. Redis AOF acknowledgement can make this take about a minute.
Each run creates a new UUID namespace and never overwrites an earlier run.

With the supplied configuration, check:

| Field | Expected |
| --- | --- |
| event | nautilus_paper_complete |
| execution_engine_started | true |
| native_execution_client | true |
| quotes / signals / paper_fills | 15 / 2 / 2 |
| cancelled | 1 (explicit nonmarketable cancellation probe) |
| native_closed_positions / open_contracts | 1 / 0 |
| native_events | 12 |
| native_replay_verified / checkpoint_verified | true / true |
| broker_accessed / live_orders_enabled | false / false |

## Implemented route

The existing crossover consumes native quotes through the application runner.
The DataEngine receives each quote. Strategy signals become native LIMIT/DAY
SubmitOrder commands. ExecutionEngine routes them to PaperExecutionClient, a real
Nautilus ExecutionClient implementation registered for MCX with netting OMS.

A bounded worker queue separates Redis I/O and paper matching from the native
engine thread. The runner waits for each response as a deterministic verification
barrier; this is not a throughput benchmark or an asynchronous live node.

The worker uses the existing Kite intent translation, order journal and submission
budget coordinator with a mock broker. It publishes native Submitted and Accepted
events only after journal and event-outbox AOF acknowledgement. The simulated
venue owns acceptance; real Kite OMS acknowledgements retain their existing,
different semantics.

A later valid quote can fill a one-contract limit order at the opposite best
price. Native Filled events update the Nautilus order and position cache.
Explicit native CancelOrder generates a durable Canceled event. The final
checkpoint is checked in Redis. A fresh cache and ExecutionEngine replay the
persisted initialized/order events without any registered execution client, and
verify terminal order statuses and positions against the original run.

## Modules and persistence

- kite-paper/client: native client, lifecycle and unsupported-operation errors.
- kite-paper/validation: order restrictions and quote-quality checks.
- kite-paper/worker: bounded worker, matching, journal and checkpoint writes.
- kite-paper/events: native event construction and paper identity.
- kite-paper/outbox: Redis CAS event publication log, fail-closed writer.
- kite-node/native_paper_command: native engine/strategy verification runner.

Redis keys retain the existing simulation namespace:

- susanta:nautilus:sim:journal:{UUID}
- susanta:nautilus:sim:order-budget:{UUID}
- susanta:nautilus:sim:strategy:{UUID}
- susanta:nautilus:sim:native-events:{UUID}

The UUID identifies a separate run; it is not an API key or access token.
Paper mode requires no Kite credentials and has no HTTP order transport dependency.

## Scope and recovery

This command uses a synthetic quote fixture and the previously verified September
2026 MCX contract specification. Its fixed instrument token/date are fixture data,
not current instrument discovery. It does not consume the live Kite socket.
The existing capture/replay commands and Step 6 strategy-replay remain available.

Only one pending order and at most one long contract are supported. Native paper
modification, order lists, batches, reconciliation queries, partial fills, queue
priority, fees and margin risk are not implemented. Unsupported client operations
return errors. The synthetic account balance is fixed; zero commissions and native
position calculations do not validate real account economics.

This integrates the actual ExecutionEngine and ExecutionClient. The crossover
remains the existing Rust strategy component called by the runner; it is not yet
a Nautilus Strategy actor or a full LiveNode. The real Kite HTTP transport remains
separately guarded and is never called here.

Journal, checkpoint and outbox are separate Redis transactions. A crash between
them may leave a journal decision without a published native event. The worker
stops on uncertainty, and restarting an existing namespace is rejected. Replay is
read-only verification, not automatic trading resumption. Inspect/reconcile all
three stores before any future resume feature; never resend merely because an
event is absent. A bounded queue failure also stops progression.

## Automated verification

```bash
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Tests use isolated Redis servers with AOF, including native engine roundtrip,
Redis restart, stale quote rejection, duplicate submission, outbox conflict and
native quantity rejection. They do not contact Kite.

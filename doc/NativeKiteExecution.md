# Native Kite adapter integration

Latest hardening, short trading, explicit signals, local protection and official sandbox status: [NativeHardening.md](NativeHardening.md). Real orders remain disabled.

Updated 15 September 2026. Kite native APIs only. Real broker orders stay disabled.
The supported LIMIT/DAY execution flow is integrated and tested through LiveNode
with a deterministic Kite broker fixture. The requested hardening is implemented; see NativeHardening.md for the current review checkpoint.

## Run the integrated adapter fixture

From /home/ubuntu/RustNautilasProject:

    cargo run --locked -p kite-node -- native-kite-mock config/strategy-crossover.toml

This registers the adapter through LiveNodeBuilder.add_exec_client, runs the native
strategy/risk/execution engines, persists command ownership and native events in
Redis, and captures complete packets in the native catalog. It does not load real
credentials or contact a broker. The mock broker supplies deterministic fills at
requested limits; use native-backtest for the native matching-engine backtest.
Synthetic mock ticks are paced at 3 seconds so Redis AOF acknowledgements complete
before the fixture's next crossover. Other simulation commands retain 500 ms ticks.

The verified fixture produces 15 ticks, two signals, two fills and a flat position.
Native Redis reconstruction verifies Initialized, Submitted, Accepted, Filled for
both orders. A separate command journal verifies both durable broker-ID/tag mappings.

## Components

- execution/native.rs: exact native LIMIT/DAY translation; fractional prices or
  quantities and unsupported instructions fail. Reduce-only exits require a
  matching broker/native position and cannot increase or reverse exposure.
- execution/broker_events.rs: validates ownership, contract/product/side, quantities,
  trade identity and chronology before emitting any native events. HTTP receipt is
  not acceptance. Partial fills precede cancellation; repeated observations do not
  duplicate fills. Exchange timestamps are explicitly UTC+05:30.
- execution/native_client/ledger.rs: Redis command ownership, immutable namespace,
  compare-and-set updates, broker tag association and WAITAOF confirmation. Attempt
  state is persisted before transport. There is no automatic command resubmission
  or restart/resume. Persistence uncertainty stops further dispatch.
- execution/native_client/dispatch.rs: serial submit/cancel coordination, broker
  position preflight, acknowledgement tracking, lost-ack correlation and event
  publication. Broker IDs from observations must agree with persisted ownership.
- execution/native_client/mod.rs: native ExecutionClient, factory, lifecycle,
  delayed-update polling, order/fill/position reports and mass reconciliation.
- execution/native_client/broker.rs: actual Kite profile, order, trade, position and
  commodity-margin APIs. Orders are read before and after the other observations
  to reject a changing snapshot. Identity is checked against the expected user.
- execution/native_client/fees.rs: Kite virtual-contract-note charge calculation.
  Calculated order charges are allocated by fill notional with a deterministic
  final-cent remainder. HTTP request/response identity and rounding are tested.
- execution/native_client/mock.rs: a broker fixture using the same dispatcher and
  native client. Its execution cannot access the network. Fixture commissions are
  explicitly zero; this is not a claim about real brokerage charges.

## Execution gates and configuration

The real Factory creates a read-only client: native submit emits Denied and all
real broker mutations remain unavailable. Its Config holds the expected user ID,
MIS/NRML product, resolved instrument token and redacted credentials. MockFactory and the separate fixed-host SandboxFactory attach a dispatcher to a runnable client. Enabling the old
adapter live-orders Cargo feature does not enable real orders in this native client.

The usual native-node-sim/native-node-paper commands continue to use
Environment::Sandbox and SandboxExecutionClientFactory in native_node/runner.rs.
native-kite-mock selects MockFactory. native-node-live remains rejected in
native_node/cli.rs. There is no usable native real-order enable flag or live-order
configuration file. Keep these gates closed during subsequent hardening.

Strategy edit point: apps/kite-node/src/native_node/strategy.rs.
UserStrategy.on_full_tick receives full data; quote-only replay uses on_quote.
Parameters: config/strategy-crossover.toml. Contract: config/crudeoil-september.toml.
The native strategy scope is now one long OR short contract, one pending order, LIMIT/DAY and an entry cap. Modification, batches, market/stop orders and new products are outside
this adapter's current execution scope; unsupported commands fail explicitly.
Synchronous query_account/query_order are not exposed; use the native asynchronous
report methods. Current-position reports reject historical time ranges. Broker
order/trade history is the current Kite session/day, not a historical archive.

## Account and fee interpretation

Commodity account total is free trading resources plus utilised resources. It is
not portfolio equity. The native account records this basis and does not request
synthetic account calculations. Unsupported negative utilised margin fails rather
than being silently clamped. Nonzero exposure in another product for this contract
is rejected; bulk position coverage is false.

Raw incremental Kite trades have no commission field, so mapped OrderFilled events
retain unknown commission. FillReport and mass reconciliation use calculated charges
from Kite's virtual contract note. These are calculated allocations, not exchange-
reported per-fill commissions or a final settled contract note. Missing or invalid
calculated charges fail the report, with no fabricated zero charges. Native-owned
broker orders resolve back to persisted client IDs; tags alone do not confer ownership.

## Verification and next phase

Workspace tests cover native engine execution, native cache replay, the Redis command
journal, failed persistence, duplicate attempts, lost acknowledgements, reduce-only
limits, delayed fills, cancellation, broker identity and native report ownership.
Fee tests use a local HTTP server and verify the fixed charges path, authentication,
numeric JSON payload, response identity and exact charge allocation.
No actual broker requests were made for this new adapter's tests. The older bounded
live-data runs used simulated execution and remain separately documented.

Latest retained logs:
- /tmp/kite-native-only-tests.log
- /tmp/kite-native-only-clippy.log
- /tmp/kite-native-client-feature-tests.log
- /tmp/kite-upstream-execution-tests.log
- /tmp/kite-upstream-twap-tests.log

Native event compatibility fixes and upstream gates: NativeEventCompatibility.md.
The next step is manual code review and authenticated official sandbox validation.
See NativeHardening.md for the implemented controls, verification and remaining
operational limits. Real-order activation remains separately disabled.

Primary API contracts:
- https://kite.trade/docs/connect/v3/orders/
- https://kite.trade/docs/connect/v3/user/
- https://kite.trade/docs/connect/v3/portfolio/
- https://kite.trade/docs/connect/v3/margins/#virtual-contract-note

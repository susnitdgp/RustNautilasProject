# Step 5A: Redis order journal and execution simulation

User requirement: use Redis for all order and application-state persistence.
SQLite is not used. Parquet remains the market-data recording format.

## Verify on the development server

```bash
cd /home/ubuntu/RustNautilasProject
cargo run --locked -p kite-node -- execution-sim
```

Each run creates a new UUID namespace, so this command can be repeated.
An explicit namespace is optional:

```bash
cargo run --locked -p kite-node -- execution-sim --namespace my-check-001
```

Explicit namespaces must be new; existing journals are not overwritten.
The old --journal FILE option is removed. Existing SQLite simulation files
are unused and have not been deleted or imported; they contain only fixtures.

The output reports persistence=redis_aof and the namespace. Inside result,
expect two simulated submissions, two filled contracts, ten journal records,
duplicate_fill_ignored=true, both retry-blocking flags=true,
broker_accessed=false and live_orders_enabled=false.

## Components

| Component | Module |
| --- | --- |
| Redis configuration and AOF confirmation | kite-journal/src/connection.rs |
| Intent and event definitions | kite-journal/src/model.rs |
| Order state and ownership validation | kite-journal/src/state.rs |
| Redis atomic journal and recovery | kite-journal/src/store.rs |
| LIMIT/DAY translation | kite-execution/src/translation.rs |
| Mock broker | kite-execution/src/mock.rs |
| Persist-before-submit coordinator | kite-execution/src/coordinator.rs |
| Verification scenario | kite-execution/src/verification.rs |

All paths above are under crates/. The command is in
apps/kite-node/src/execution_command.rs.

## Redis settings and key layout

The journal reads KITE_REDIS_URL, defaulting to redis://127.0.0.1:6379/0.
It uses a separate Redis connection module and never reads broker credentials.
The existing credential module still reads susanta:kite_api_key and
susanta:kite_access_token. Neither their values nor the Redis URL is printed.

Journal key: susanta:nautilus:sim:journal:{NAMESPACE}
It is a single hash containing scope, generation UUID, revision, and
event:1 through event:N. State is rebuilt from validated, ordered events.
There are no journal key expirations. Namespaces are restricted to 1..64
ASCII letters, digits, hyphens and underscores.

Redis must be version 7.2 or newer, with appendonly=yes and
maxmemory-policy=noeviction. The connection checks noeviction and requires
WAITAOF. CONFIG GET maxmemory-policy permission is needed, along with hash
commands, EVAL and WAITAOF. Runtime code does not change server configuration.

On this development server, AOF was disabled. During this change appendonly
was enabled and CONFIG REWRITE succeeded. appendfsync remains everysec.
Each journal creation/append explicitly uses WAITAOF 1 0 2000 on the same
connection and requires one local AOF acknowledgement before returning.
Thus a dispatch cannot proceed merely on an in-memory write acknowledgement.

This is local disk acknowledgement, not replicated durability or protection
against loss of the Redis host/storage. Disk and OS must honor sync operations.
Replica failover and production account-wide coordination remain later work.

## Atomicity and failures

A Lua compare-and-set verifies scope, generation, expected revision, field
count, absent next event and no TTL. It appends the event and new revision
in a single HSET command. A generation UUID prevents a stale handle from
writing after a namespace is deleted and recreated.

Multiple readers may open a namespace. A writer with a stale revision fails
before the coordinator calls the mock broker. A failed write or AOF confirmation
poisons the handle: no further writes are allowed until reopening and reviewing
recovered state. An uncertain Redis response is not retried automatically.
The database might contain that event even if the client did not get confirmation.

Recovery takes an atomic snapshot, validates metadata/counts/sequence/version
and replays state transitions. Corrupt or missing records, wrong scope, TTLs
and unsupported schemas fail closed. No silent repair or key deletion occurs.
The scope KITE_SIMULATION_ONLY_V2 is explicitly synthetic, not a real account.

Limits: 10,000 events per namespace, 8 KiB per event. Recovery loads state into
memory and append validation clones it. Redis calls block only the standalone
simulation command; they are not wired into the Nautilus core event loop.

## Execution behavior

Prepared -> Dispatching is persisted and AOF-confirmed before a mock submission.
An acknowledgement is not a fill. Ambiguous timeout becomes Unknown.
Dispatching and Unknown prevent blind retries after restart.

Partial fills use unique trade IDs and integer prices. Exact duplicates are
ignored; conflicting duplicates, overfills, broker-ID conflicts and limit-price
violations fail. Early fills can precede acknowledgements. Late acknowledgements
or timeouts cannot regress a known fill. Delayed valid trades after cancellation
remain supported up to the intended total quantity.

The coordinator only accepts MockBroker. It has no live order transport.
Translation is fixed to the CRUDEOIL26SEPFUT simulation contract, regular
LIMIT/DAY, MIS/NRML and BUY/SELL. Quantity is contracts, not barrels. The
6000-rupee price and quantities are fixtures, not market data or trade advice.
The simulation quantity bound does not establish current broker eligibility.

## Tests and manual result

Integration tests launch isolated Redis servers with AOF, temporary directories,
random local ports and synthetic state. They never flush the shared Redis or
read credentials. Redis-server 7.2+ must be installed to run these tests.

Coverage includes:
- Application child-process exit after a durable dispatch, followed by retry blocking.
- Redis process kill and recovery from AOF.
- Duplicate fills/intents, partial fills, early updates and cancellation.
- Invalid identities, prices, quantities and conflicting trade IDs.
- Concurrent stale writers and deletion/recreation generation checks.
- Corrupt records, sequence gaps, TTLs and missing namespaces.
- Disabled AOF rejection before creation and poisoned handles after failed writes.

The process-exit helper is marked ignored in normal discovery; its parent
test explicitly launches it. These tests are not a physical power-loss test.

All 80 workspace tests and Clippy with warnings denied passed.
Two consecutive CLI runs against development Redis completed with different
namespaces and the expected ten records each.

Step 4's cancelled-order quantity fix remains included: pending quantity can
remain populated on terminal Kite orders and is not blindly added to cancelled
quantity. No broker mutation calls were made.

## Remaining work

Production account/date scope, ambiguity resolution against broker records,
native Nautilus execution reports, rate limiting, bounded queues, cancel/modify
requests, production transport and strategy/risk controls remain pending.
Order/state persistence for those components must also use Redis.

Reference: [Redis WAITAOF](https://redis.io/docs/latest/commands/waitaof/).

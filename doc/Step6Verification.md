# Step 6: reference strategy, paper replay and guarded Kite execution transport

Step 5D was reviewed, committed as dcabeee and pushed before this work began.
This checkpoint adds executable strategy logic and the Kite order HTTP/service
modules. It does not enable live trading or claim production readiness.

## Verify the strategy

```bash
cd /home/ubuntu/RustNautilasProject
cargo run --locked -p kite-node -- strategy-sim config/strategy-crossover.toml
```

Expected: quotes=15, signals=2, paper_fills=2, cancelled=0,
open_contracts=0, checkpoint_verified=true, broker_accessed=false and
live_orders_enabled=false.

To use the earlier recorded MCX quotes:

```bash
cargo run --locked -p kite-node -- strategy-replay config/strategy-crossover.toml --input data/step3-verification-20260915.parquet
```

The capture is validated before Redis state is created. A replay is limited
to 2000 records. Real capture results depend on its prices; zero signals is a
valid result and is not replaced with fabricated trades.

## Strategy and paper execution modules

| Component | Source |
| --- | --- |
| Configuration | crates/kite-strategy/src/config.rs |
| Crossover logic | crates/kite-strategy/src/crossover.rs |
| Redis strategy checkpoint | crates/kite-strategy/src/checkpoint.rs |
| Paper matching and Parquet replay | crates/kite-strategy/src/paper.rs |
| CLI | apps/kite-node/src/strategy_command.rs |
| Initial parameters | config/strategy-crossover.toml |

This is a reference quote-midpoint SMA crossover, not a profitability claim.
The default fast/slow periods are 3 and 5 quotes, not minutes or candles.
After warmup, an upward crossover enters one long contract; a downward
crossover exits an existing long. It does not short or pyramid.

Only one order may be pending. Position changes follow recorded fills, not
signals or OMS acknowledgements. The entry cap defaults to 20 filled entries.
The exact Decimal calculations avoid floating-point indicator arithmetic.

Bad instrument identity fails. Wide spreads, insufficient top-level size,
stale timestamps, backward event time and off-tick prices reset indicator
warmup and suppress trading. Gaps cancel the paper pending order and reset
warmup. Default maximum spread is two rupees and maximum receipt delay is
ten seconds, evaluated using the recorded timestamps during replay.

A paper signal submits a one-contract LIMIT order at the quoted ask for a buy
or bid for a sell, through the existing Redis journal, rate budget and MockBroker.
A fill can only occur on a subsequent valid quote that crosses that limit.
A pending order is cancelled after three unmatched quotes, an invalid quote,
a gap or end of input. This paper cancellation is synthetic.

The model assumes one-contract liquidity at the displayed top of book.
It does not model exchange queue position, latency, brokerage, taxes or market
impact. Remaining filled positions are reported as open_contracts; an exit is
not invented at end of replay. Session-end flattening and production risk
controls remain to be implemented before live strategy use.

## Redis persistence

Strategy checkpoints use:
susanta:nautilus:sim:strategy:{NAMESPACE}

The existing journal and order-budget keys use the same namespace. Each CLI
run generates a fresh namespace and preserves previous results. Checkpoints
store configuration, rolling indicator values, last timestamp, position,
pending state, entry count and replay cursor. Writes use revision comparison,
no TTL and Redis AOF acknowledgement.

A pending decision is checkpointed before an order is journaled/dispatched.
The final checkpoint is read back and the position is checked against a
reopened order journal. A checkpoint failure stops the run.

Checkpoint and journal updates are separate Redis operations, not a combined
transaction. Automatic crash continuation is deliberately not implemented:
an interrupted run requires reconciliation before resuming, especially between
the pending-decision checkpoint and intent persistence. Saved state supports
review, not automatic resubmission. All order/state persistence remains Redis.

## Kite execution modules

| Component | Source |
| --- | --- |
| Validated place/modify/cancel commands | crates/kite-adapter/src/execution/request.rs |
| HTTP transport and outcome classification | crates/kite-adapter/src/execution/transport.rs |
| Journal/rate-budget coordination | crates/kite-adapter/src/execution/service.rs |

Implemented wire operations:
- POST /orders/regular for regular LIMIT/DAY orders.
- PUT /orders/regular/{order_id} for quantity/price changes.
- DELETE /orders/regular/{order_id} for cancellation.

The transport uses fixed official Kite endpoints, form-encoded fields,
X-Kite-Version: 3 and sensitive Authorization headers. Credentials are supplied
as the existing KiteCredentials type, which the separate Redis credential
module loads. Tests use synthetic credentials only.

Input validation covers the supported MCX symbol, MIS/NRML, BUY/SELL, bounded
contract quantity, positive whole-rupee prices, tags and numeric broker IDs.
No market, stop, iceberg, basket or automatic slicing support is added.

Connect timeout is three seconds; total request timeout is five seconds.
Redirects and reqwest automatic retries are disabled. Success bodies are bounded
to 64 KiB and zeroized after parsing. No raw error bodies, credentials or
connection URLs are included in errors.

An acknowledgement is not a fill or modification/cancellation confirmation.
Malformed success, mismatched order IDs, connection loss and server/proxy
failures produce Unknown. Authentication failure stops for renewal/reconciliation.
429 produces a bounded cooldown using integer Retry-After seconds or a
conservative ten-second fallback; HTTP-date Retry-After parsing remains pending.
Ordinary input/not-found/method validation errors are classified as rejected.
No ambiguous mutation is retried automatically.

The service validates and reserves capacity, persists Dispatching, then invokes
the transport. It records the response in Redis. Management requests retain
the separate acknowledgement/confirmation lifecycle. A session/transport
failure records Unknown and returns an error.

## Live gate and remaining integration

The normal build excludes the adapter's live-orders Cargo feature.
Both transport and service reject live calls before sending any request.
The application exposes only paper strategy commands; there is no live-order
CLI. This checkpoint has not enabled that feature or called broker mutations.

The feature is an implementation gate, not proof of live readiness. Production
account/date identity, fresh broker reconciliation, real postback/trade capture,
fees/positions, dispatch timing, session risk controls and authorization must
be completed before use. Existing journals/budgets are simulation-scoped and
must not be repurposed as production account state.

The strategy consumes native QuoteTick values but is not yet registered as a
Nautilus Strategy actor in a full LiveNode. The execution service is not yet a
Nautilus ExecutionClient. Native execution reports from Step 5D are available,
but live execution-engine routing and broker observation ingestion remain pending.
No market-data or broker callback is being silently treated as an execution fill.

## Verification

The workspace suite passed with the new strategy and transport tests. After
adding explicit no-retry behavior, all four focused transport tests and Clippy
with warnings denied passed. There are 113 tested cases across these runs;
the existing child-process helper remains separately invoked by its parent.

Strategy tests cover warmup, crossover, pending protection, stale/wide/backward
quotes, gaps, position constraints, configuration, subsequent-quote matching,
end-of-input cancellation and Redis position agreement.

Transport tests use an isolated localhost HTTP server to check POST/PUT/DELETE,
headers, forms, acknowledgement IDs, 400/403/429/502, malformed/mismatched success,
the disabled build gate and a dropped response without a second request.
No test contacts Kite for order submission.

The strategy-sim CLI completed on development Redis with two signals, two paper
fills and a flat final position. The earlier MCX Parquet capture was also replayed.
See terminal summary for signal/fill counts, which depend on the recorded prices.

Reference: [Kite order API](https://kite.trade/docs/connect/v3/orders/) and
[Kite errors](https://kite.trade/docs/connect/v3/exceptions/).

Changes are uncommitted for manual review.

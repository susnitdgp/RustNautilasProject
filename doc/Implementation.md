# Implementation stages

Current project: /home/ubuntu/RustNautilasProject on ip-172-31-36-59.
Original Step 1 host: vmi3506951.
Target changed from the initial NSE proposal to standard MCX CRUDEOIL September
2026 futures at the user's direction. Live execution remains disabled.

## Component boundaries

Each component gets its own Rust module. Modules appear only when implemented;
empty placeholders are not presented as finished components.

| Component | Location | Stage |
| --- | --- | --- |
| Target configuration | crates/kite-adapter/src/config.rs | 1 |
| Instrument HTTP download | crates/kite-adapter/src/http/instruments.rs | 1 |
| CSV master parsing | crates/kite-adapter/src/instruments/master.rs | 1 |
| Contract selection | crates/kite-adapter/src/instruments/resolver.rs | 1 |
| Nautilus identity mapping | crates/kite-adapter/src/mapping/identity.rs | 1 |
| Read-only preflight orchestration | crates/kite-adapter/src/preflight.rs | 1 |
| CLI entrypoint | apps/kite-node/src/main.rs | 1 |
| Redis credential loading | adapter credentials/redis module | Implemented |
| Session validation | adapter auth/session module | Implemented; renewal deferred |
| Binary frame parsing | adapter websocket/parser module | Implemented |
| Socket/reconnect/subscriptions | adapter websocket transport/supervisor/subscription modules | Implemented |
| Quote/depth observations | adapter mapping/market_data module | Implemented; native quote mapping available |
| Nautilus DataClient/factory | adapter data/factories modules | Implemented for quotes |
| DataEngine runner lifecycle | application runtime modules | Implemented; full LiveNode later |
| Recording and quote replay | separate kite-recorder crate | Implemented |
| Account and product mapping | adapter account module | Implemented read-only; native account reports deferred |
| Orders, trades and positions | separate adapter orders/trades/positions modules | Implemented read-only |
| Broker snapshot reconciliation | adapter reconciliation module | Implemented target quantity consistency |
| Redis order journal | separate kite-journal crate | 5A implemented, simulation scope |
| Order command translation | kite-execution translation module | 5A regular LIMIT/DAY simulation |
| Persist-before-submit and mock broker | kite-execution coordinator/mock modules | 5A implemented |
| Native order/fill report mapping | kite-execution reports modules | 5D simulation mapping implemented; engine integration pending |
| Mock modify/cancel lifecycle | journal actions and execution management modules | 5C implemented; real broker transport pending |
| Shared order-request budgets | kite-execution rate_limit modules | 5B simulation implemented; production dispatcher pending |
| Strategy | separate kite-strategy crate | 6 reference crossover and paper replay implemented |
| Kite order HTTP/service | adapter execution modules | 6 implemented with live gate disabled; engine integration pending |
| Native paper ExecutionClient and Redis worker | separate kite-paper crate | 7 implemented, synthetic venue |
| Native ExecutionEngine strategy runner | application native_paper_command module | 7 implemented; full Strategy actor/LiveNode pending |
| Application trading controls | separate risk-controls crate | Planned; current paper checks are scoped to simulation |

## Checkpoints

1. Foundation: build/test; manually compare resolved symbol, token, expiry and
   tick size against Kite. This does not validate quantities or live execution.
2. Authenticated data: user supplies credentials through a protected local
   environment, never chat. Verify fresh bid/ask/LTP, timestamps, disconnect
   behavior and no duplicate subscriptions. No orders.
3. Nautilus runtime and recording: verify callbacks, gap markers, replay and
   restart. Construct a full futures instrument only after the official MCX
   multiplier and Kite quantity semantics are checked.
4. Account/reconciliation: read-only account snapshots; verify product-aware
   positions and external/manual orders. No submissions.
5. Execution implementation: mock broker tests for timeouts, early updates,
   duplicates, partial fills and crash recovery. No live orders.
6. Strategy/risk validation: replay and simulation; then separately authorized
   bounded live execution checks with explicit order parameters.

## Dependency decision

Pin nautilus-model 0.63.0 and Rust 1.98.0 with a committed Cargo.lock.
The upstream develop checkout inspected during discovery required Rust 1.98.1.
No files from that checkout are project dependencies. The unrelated
/opt/rust-nautilus discovery checkout is not used by builds.

Do not hard-code an instrument token or treat instrument-master lot_size as a
monetary multiplier. Metadata can change; resolve from the current master.
The offline CSV route is explicitly reported as freshness-unverified.

Step 1's blocking HTTP client runs only in the standalone preflight CLI.
The future live runtime must use async transport and must not call this
blocking downloader from Nautilus's event loop.

Step 2 automated and live diagnostic checks have passed; manual verification
is described in doc/Step2Verification.md. Session renewal is not implemented.

Step 3 capture and replay verification passed; see doc/Step3Verification.md.
The current artifact format is application Parquet, not the native Nautilus catalog.

Step 4 automated and live read-only checks passed; see doc/Step4Verification.md.
This stage does not hydrate the Nautilus execution engine or enable orders.

Step 5A journal and offline mock execution checks passed; see doc/Step5Verification.md.
The rest of Step 5 remains pending, including production ownership, execution
reports, rate limiting, cancel/modify requests and live transport.

Step 5B shared Redis order-budget tests passed; see doc/Step5BVerification.md.
Strategy logic and real Kite order execution are not implemented.

Step 5C Redis-backed mock modification/cancellation checks passed; see
doc/Step5CVerification.md. Strategy and real Kite order execution remain pending.

Step 5D native Nautilus order/fill report mapping checks passed; see
doc/Step5DVerification.md. This does not start an execution engine or enable live orders.

Step 6 reference strategy, Redis checkpoints, paper matching and guarded Kite
order transport/service are implemented; see doc/Step6Verification.md.
Full LiveNode and production broker/risk integration remain pending.

## Step 7 native paper execution

Real Nautilus ExecutionClient/ExecutionEngine paper integration is implemented.
See [Step 7 verification](Step7Verification.md) for the command, expected output,
Redis event replay and remaining live-node/recovery limitations.

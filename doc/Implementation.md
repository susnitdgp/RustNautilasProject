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
| Session lifecycle | adapter auth module | 2 |
| Binary frame parsing | adapter websocket parser module | 2 |
| Socket/reconnect/subscriptions | adapter websocket transport module | 2 |
| Quote/depth mapping | adapter mapping data module | 2 |
| Nautilus DataClient/factory | adapter data/factories modules | 3 |
| Node lifecycle | application runtime module | 3 |
| Recording | separate recorder crate | 3 |
| Account and product mapping | adapter account module | 4 |
| Broker snapshot reconciliation | adapter reconciliation module | 4 |
| Durable order journal | separate journal crate | 5 |
| Order command translation | adapter execution module | 5 |
| Account-wide rate limiting | adapter rate_limit module | 5 |
| Strategy | separate strategies crate | 6 |
| Application trading controls | separate risk-controls crate | 6 |

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

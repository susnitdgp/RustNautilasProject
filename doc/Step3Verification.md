# Step 3: Nautilus quote integration, Parquet recording and replay

Current project: /home/ubuntu/RustNautilasProject on ip-172-31-36-59.
Step 2 was reviewed and pushed as e9517ea before this work.

## Implemented components

| Component | Path |
| --- | --- |
| Verified September crude-oil specification | crates/kite-adapter/src/instruments/contract.rs |
| Nautilus QuoteTick mapping | crates/kite-adapter/src/mapping/quotes.rs |
| Data-client configuration | crates/kite-adapter/src/data/config.rs |
| Bounded adapter event interface | crates/kite-adapter/src/data/events.rs |
| Nautilus DataClient implementation | crates/kite-adapter/src/data/client.rs |
| Nautilus DataClientFactory implementation | crates/kite-adapter/src/factories.rs |
| DataEngine, cache and quote callback | apps/kite-node/src/runtime/core.rs |
| Live capture orchestration | apps/kite-node/src/runtime/capture.rs |
| Offline DataEngine replay | apps/kite-node/src/runtime/replay.rs |
| Recording schema | crates/kite-recorder/src/records.rs |
| Parquet writer thread | crates/kite-recorder/src/writer.rs |
| Capture integrity and replay reader | crates/kite-recorder/src/replay.rs |

This is a data-only Nautilus DataEngine runner with a real DataClient and factory.
It is not yet a full LiveNode/kernel trading deployment. There is no strategy,
RiskEngine, ExecutionEngine or account/portfolio reconciliation in this runner.
The factory receives the application-owned bounded event sink in its configuration;
integration with a generic LiveNode event emitter is not yet provided.

## Verified contract mapping

For CRUDEOIL26SEPFUT.MCX:
- 100 barrels per contract, quoted in INR per barrel.
- Quantity 1 means one contract; multiplier 100 must be kept separate.
- Tick size INR 1 per barrel: INR 100 price-tick value per contract.
- Launch date 20 March 2026; expiry date 21 September 2026.
- Activation uses 09:00 IST; expiration uses the September session end, 23:30 IST.
- Minimum size and lot increment in the Nautilus model are one contract.

The mapper rejects other contracts and unexpected broker tick/lot metadata.
It does not infer the monetary multiplier from the instrument-master lot_size.
Exchange MIC is not invented. Cash settlement and source provenance are retained
in instrument info. Margin and fee fields remain unconfigured framework defaults,
explicitly marked in metadata; this instrument must not be used for margin,
fee or execution decisions until those later components are implemented.

Sources:
- [MCX contract specification and 2026 calendar](https://www.mcxindia.com/docs/default-source/products/contract-specification/crude-oil/crude-oil-january-2026-contract-onwards267be8c1-650a-4baa-aabd-ffcc9364c100.pdf)
- [Kite staff explanation of MCX quantity](https://kite.trade/forum/discussion/14531/lot-size-of-all-mcx-instruments-are-1)

## Data flow and threading

1. Standalone startup resolves the master and loads Redis credentials.
2. The runner builds the instrument and registers it through DataEngine into cache.
3. The factory constructs KiteDataClient using the read-only cache view.
4. Client start/connect validates the session and opens the real WebSocket.
5. A Nautilus quote subscription starts the socket worker using that connection.
6. Tokio workers produce owned QuoteTick and connection/gap events into a bounded
   1,024-item channel. They never access core cache or MessageBus references.
7. The main runtime thread passes quotes through DataEngine, which updates its
   cache and invokes the typed MessageBus callback.
8. A separate writer thread receives records through a bounded 1,024-item queue.

The main future remains on the thread driving Runtime::block_on. Core state uses
Rc/RefCell and is not moved to the spawned network workers. Unsupported data
capabilities return errors instead of accepting the trait's default no-op behavior.

Missing, one-sided, zero-size or stale observations are not emitted as QuoteTick.
Crossed or off-tick quotes fail the capture. Accepted quotes preserve source
nanoseconds and receive nanoseconds, with integer contract quantities.
Only normalized top-of-book quotes and connection/gap events are recorded here.
Raw WebSocket frames, LTP-only observations and full five-level books are not
recorded by this stage; the Step 2 diagnostic remains available separately.

A full channel, mapping error or writer error fails the capture instead of
reporting complete data. Stop cancels network work; disconnect awaits the worker
before reuse. A new socket generation must supply fresh full-mode data.

## Recording format and integrity

Files are genuine Snappy-compressed Parquet. Each row has sequence, kind and
JSON payload columns. The typed payload stores a versioned instrument header,
connection/gap events, QuoteTick records and an explicit completion count.
This is an application capture format, not the native Nautilus Parquet catalog.

The writer batches up to 256 rows and finalizes/fsyncs the file on completion.
There is no per-tick durability guarantee; an abrupt crash can leave an incomplete
file. Existing files are never overwritten. Choose a new path for each run.

Replay checks sequence, schema version, instrument identity, generation transitions,
completion count and trailing records. Missing footer/completion fails validation.
The CLI validates before feeding the capture through the DataEngine. Replay is
quote transport replay, not strategy backtesting or virtual-clock timer replay.
Source and receipt timestamps are preserved; gap events remain explicit.

## Verification completed

On 15 September 2026:
- 56 automated tests passed with zero failures.
- Clippy passed with warnings denied.
- Formatting and whitespace checks passed.
- A 15-second live capture accepted 16 quotes.
- All 16 generated Nautilus MessageBus callbacks and were recorded.
- Zero gaps; the final cached quote matched the last processed quote.
- Offline replay produced the same 16 callbacks from 19 total records.
- Replay accessed no broker or Redis.
- Live orders remained disabled.

Existing verification capture:
data/step3-verification-20260915.parquet (2,485 bytes).

## Your manual checkpoint

```bash
cd /home/ubuntu/RustNautilasProject

cargo test --locked --workspace

cargo run --locked -p kite-node -- capture \
  config/crudeoil-september.toml --seconds 15 \
  --output data/manual-step3.parquet

cargo run --locked -p kite-node -- replay data/manual-step3.parquet
```

Run capture during an active feed. Both commands must exit successfully.
Compare recorded_quotes from capture with quotes from replay and with
data_engine_callbacks from both commands; all counts should agree.
Replay records includes header/connection/completion rows as well as quotes.
On a normal uninterrupted capture, records = quotes + 3.

Use a new output filename if manual-step3.parquet already exists.
The program records all accepted quotes, unlike the five-sample Step 2 display.
A stale/empty feed produces a failed verification and no complete-capture claim.
Read doc/Step2Verification.md for a side-by-side live price check in Kite.

## Next stage

After manual verification: account/product mapping and read-only broker
reconciliation. Live execution remains disabled until dedicated command lifecycle,
journal, rate-limit and risk-control components have been implemented and tested.

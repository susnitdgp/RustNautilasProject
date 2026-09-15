# Rust Nautilus + Zerodha Kite

Latest hardening, short trading, explicit signals, local protection and official sandbox status: [doc/NativeHardening.md](doc/NativeHardening.md). Real orders remain disabled.

Rust workspace for a modular Kite integration. First target: standard MCX
CRUDEOIL September 2026 futures (not CRUDEOILM).

**Current native integration:** LiveNode/Kernel/Trader, native strategy and audit
actor, DataEngine/RiskEngine/ExecutionEngine, Sandbox matching, portfolio/accounts,
Redis cache, ParquetDataCatalog, BacktestNode/BacktestEngine, SMA indicators,
OrderEmulator and TWAP are exercised. Full Kite packets reach the user strategy,
roundtrip through the native catalog, and replay through BacktestNode.
Native Kite ExecutionClient/factory, Redis-owned command dispatch, delayed broker
updates and calculated-fee mass reconciliation are now integrated and fixture-tested; see [Kite continuation](doc/NativeKiteExecution.md).
Run native-kite-mock to exercise this adapter through LiveNode. The real broker
client remains read-only and real orders remain disabled. Native startup and immediate LIMIT event-order warnings are
fixed with two pinned local Nautilus patches; see
[event compatibility verification](doc/NativeEventCompatibility.md).
See [native architecture and verification](doc/NativeIntegration.md) for exact
scope, test evidence, commands and remaining integration work.

Edit strategy decisions in `apps/kite-node/src/native_node/strategy.rs`.
`on_full_tick` receives depth, OHLC, volume, OI and the derived native quote;
its default delegates to `on_quote`. Quote-only catalogs use `on_quote`.
The runtime still restricts execution to one long contract and one pending order.
Strategy parameters are in `config/strategy-crossover.toml`.
There is **no usable live execution enable flag** for the native node today.
`native-node-live` rejects execution; `native-node-paper` means live data with
simulated fills, not broker orders.

**Earlier stages:** modular preflight, Redis credentials, session validation,
WebSocket diagnostics, a Nautilus DataClient/factory and DataEngine quote runner,
plus Parquet recording, offline quote replay and read-only broker reconciliation. Nautilus is pinned to 0.63.0.
See [Step 3 verification](doc/Step3Verification.md) for capture/replay commands
and [Step 2 verification](doc/Step2Verification.md) for sample price display.
See [Step 4 verification](doc/Step4Verification.md) for the read-only reconcile command.
See [Step 5A verification](doc/Step5Verification.md) for the durable Redis journal
and offline mock execution simulator.
See [Step 5B verification](doc/Step5BVerification.md) for shared Redis order budgets.
See [Step 5C verification](doc/Step5CVerification.md) for mock modification and cancellation.
See [Step 5D verification](doc/Step5DVerification.md) for native Nautilus order/fill report mapping.
See [Step 6 verification](doc/Step6Verification.md) for the reference crossover
strategy, paper replay, and guarded Kite HTTP/service modules.
Native LiveNode paper integration is now implemented. Production risk controls
and real broker activation remain pending. Earlier stage commands use
the previous Session/paper runtime and do not validate the new native node.
See [implementation stages](doc/Implementation.md) and [verification](doc/Verification.md).

Run from this repository:

```bash
cargo run --locked -p kite-node -- preflight config/crudeoil-september.toml --download
cargo test --locked --workspace
```

Preflight loads the API key and access token from Redis before downloading the
public instrument master. See [Redis credentials](doc/RedisCredentials.md).
Configuration requires an explicit expiry. Expired targets fail; there is no
automatic roll. Token, lot size and tick size come from the downloaded master.

The reported broker lot size is NOT the monetary multiplier. The September
standard crude-oil contract now has a source-verified 100-barrel multiplier.
MIS/NRML profile mapping and broker quantity checks are implemented.
Native account/order/fill/position reports and Redis-owned mock dispatch are
implemented. Real broker execution stays disabled pending hardening and authorization.

Nautilus dependencies are LGPL-3.0-only; review upstream license obligations
before redistribution. This project pins published crates and carries two documented local compatibility
patches; see vendor/README.md.

### Native Nautilus paper execution (Step 7)

Run `cargo run --locked -p kite-node -- nautilus-paper-sim config/strategy-crossover.toml`.
This exercises a real ExecutionEngine and paper ExecutionClient with synthetic
quotes, Redis persistence, native fills/cancellation and fresh-cache replay.
[Verification and scope](doc/Step7Verification.md).

### Live Kite full ticks with native strategy and paper risk flow (Step 8)

Run `cargo run --locked -p kite-node -- paper-flow-sim config/strategy-crossover.toml` first.
Then use `paper-live config/crudeoil-september.toml config/strategy-crossover.toml --seconds 30`.
The native Strategy actor receives KiteFullTick custom data including five-level
depth, OHLC, volume and open interest. Derived quotes support paper matching.
The RiskEngine routes orders to paper execution only; diagnostics classify rejected
updates and distinguish transport gaps from quality suspensions.
[Verification, disconnect and restart behavior](doc/Step8Verification.md).

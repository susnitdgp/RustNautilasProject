# ILRC Combined — 3-minute CRUDEOIL

**Status:** production-shadow and broker-free mock testing only. Real order submission is disabled; `live_execution_ready=false`. Selected instrument `CRUDEOIL26OCTFUT.MCX`, 1 contract, October 19, 2026 expiry. Review expiry/calendar before reuse.

## Strategy

**Setup A — liquidity sweep reversal:** previous-day or 20-bar swing liquidity sweep, displacement and VWAP/internal structure confirmation, five-bar retracement, structural stop with 0.15 ATR buffer, opposing-liquidity target, and stop moved to break-even after +1R (effective next bar). Stops outside 0.5–1.5 ATR and rewards below 1.5R are rejected.

**Setup B — continuation:** 20-bar structure break, displacement with VWAP alignment, retracement entry, 3R target and break-even at +1R. Active A blocks B and vice versa; A has priority for equal entry timestamps. This is a shadow/backtest portfolio policy, not a completed broker-order arbiter.

## Manual production-shadow use

```bash
cargo build --release --locked -p kite-node
bash deploy/verify-ilrc-production.sh
bash deploy/run-ilrc-production-shadow.sh
```

Run in a terminal; stop with Ctrl+C. The launcher checks strategy and broker live-order gates. It loads no execution client and sends no broker orders. A systemd template exists for optional future use, but is not installed or started by the manual workflow. Its installer does not start or enable it.

## Build and mock verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
bash deploy/verify-ilrc-live-integration.sh
cargo run --locked -p kite-node -- native-ilrc-mock-execution
```

The live-readiness command is intentionally fail-closed. The mock does not connect to Kite order execution.

## Historical replay commands

Create a read-only Kite historical fixture locally (fixture is not tracked in Git):

```bash
cargo run --locked -p kite-node -- native-ilrc-causal-audit config/production-ilrc.json --fetch 2026-10-07
```

With `data/ilrc-test/real-2026-10-07.json` present:

```bash
cargo run --locked -p kite-node -- native-ilrc-causal-audit config/production-ilrc.json data/ilrc-test/real-2026-10-07.json 2026-10-07
cargo run --locked -p kite-node -- native-ilrc-timed-mock config/production-ilrc.json data/ilrc-test/real-2026-10-07.json 2026-10-07
cargo run --locked -p kite-node -- native-ilrc-timed-scenario config/production-ilrc.json data/ilrc-test/real-2026-10-07.json 2026-10-07 0.5 0
```

The scenario's final two arguments are adverse slippage points per execution side and kill-switch bar index (`0` means no kill). The scenario simulates an on-disk checkpoint round-trip, restart equivalence and an in-process kill switch; it does **not** implement production Redis journals, external-order reconciliation or a broker liquidation kill switch.

## October 7 findings and limitations

The completed-trade backtest showed 5 trades and +68.93 gross points. The next-bar-open timed mock showed 5 fills, +5.80 gross points at zero slippage and about +0.80 gross points with 0.5 adverse points on entries and exits, **before transaction charges**. The entry-event audit verified historical prefix consistency, not executable intrabar fills. The timed mock does not model same-fill-candle exits, queue position, complete broker fees, partial fills, acknowledgments, downtime, or real broker position state. These simulations are not forecasts of achievable live P&L.

**Live execution is not production-ready.** Real-time causal decision delivery, broker order lifecycle, persistent Redis order state, protective order reconciliation, crash recovery, and kill-switch behavior still require implementation and testing. Do not change `live_orders_enabled` to true; do not load an execution client as part of the shadow runner.

## ILRC-to-Kite staged order gateway

The `ilrc_order_intent.rs` module now constructs validated Kite `ProtectedMarket` commands from ILRC entry events and passes them through an injected `KiteOrderGateway`. The only supplied implementation is `DryRunKiteGateway`, which validates commands without sending broker requests. Tests cover flat/reconciled-position preconditions, signal freshness, kill-switch rejection, stop/target direction and the mock submission path. There is deliberately **no live Kite gateway**, no live CLI command and no real order submission. Broker acknowledgements, protective order placement and verification, partial fills, reconciliation, Redis persistence and crash recovery remain required before implementing the real gateway.

## Live order lifecycle development (NOT operational)

`ilrc_order_lifecycle.rs` adds a pure, serializable, fail-closed state machine for entry reservations, broker acknowledgements, deduplicated fill observations, protective-stop confirmation, mismatch-triggered manual review and a latched kill switch. Unit tests cover unknown submission outcomes and checkpoint round-trips. **This does not create or send a Kite order.** The production execution dispatcher already has Redis journaling and broker snapshot functions, but ILRC has not been wired to it.

The implementation still lacks atomic Redis state persistence at each ILRC transition, a real broker submission adapter, server-confirmed protective-stop place/modify/cancel operations, partial-fill stop-quantity adjustment, correct exchange-session cutoffs, order stream outage handling, cross-process lock/recovery and crash/fault injection against Kite mock responses. Therefore `live_execution_ready` stays false, all live order gates stay disabled, and the lifecycle module is deliberately not registered with a live CLI command.

## Kite transport adapter (disabled; not live-ready)

`ilrc_kite_adapter.rs` provides a typed connection to the existing `KiteOrderTransport::execute` API, but it is not constructed or called by any ILRC CLI/runner. The transport itself refuses real orders unless the separately reviewed `live-orders` Cargo feature is enabled. The adapter rejects submissions without caller-asserted durable journaling, stop management and broker reconciliation, but these prerequisites are **not actually implemented or independently verified for ILRC**. These booleans are integration placeholders, not authorization gates. Do not instantiate this adapter with real credentials or bypass its guards. Any uncertain transport result latches the mock lifecycle kill switch and requires manual reconciliation; acknowledgement is not a fill.

**Not yet complete:** a real ILRC reconciler, Redis-atomic command journal and account lock, confirmed protective stops before declaring a managed position, partial-fill stop resizing, crash-safe recovery, and broker-mock fault injection across the full lifecycle. No operational live trading command exists.

## Integrated mock-broker replay (no live execution)

`native-ilrc-mock-broker` exercises ILRC order-intent validation, an injected non-network gateway, acknowledgement, observed fill, duplicate-fill suppression, protective-stop state confirmation, position reconciliation, and a restart serialization round-trip. Unknown/rejected acknowledgement paths latch the mock kill switch. Run with:

```bash
cargo run --locked -p kite-node -- native-ilrc-mock-broker
```

**Scope:** This tests a synthetic broker lifecycle and does not invoke the production `kite-adapter` dispatcher, connect Redis, place a Kite protective order, or establish account reconciliation. The existing internal Nautilus dispatcher has separate Redis/fill/recovery tests but requires Nautilus `OrderAny` and a live runner with cache/clock/event sender, broker snapshots, and actual stop management. Wiring the ILRC order intents directly into the Kite HTTP transport would bypass those controls and is not authorized. The live-order path remains disconnected and all gates disabled.

## ILRC to Nautilus native order conversion

`ilrc_nautilus_bridge.rs` maps post-candle ILRC entry decisions into Nautilus `OrderAny::Market` instructions (`DAY`, exactly one contract, non-reducing entry), which are compatible with `kite_adapter::execution::native::submit` and its market-protection requirement. Tests verify real type conversion and rejection of malformed risk levels, without broker connections. The bridge is not exposed as a CLI execution command.

For **operational** integration, a distinct native live runner must own a Nautilus order factory/event bus, guarded production dispatcher, Redis account lease and journal, order stream, and broker reconciliation. It must verify protective stop orders and modifications, fill-by-fill quantities, session square-off and crash recovery. Those end-to-end activities have not been implemented for ILRC or verified, and `live_orders_enabled` must remain false. A type-correct Kite command is not evidence of safe live submission.

## Protective Kite order transport support

The shared Kite request protocol now supports validated regular `SL-M` stop-market placement (`ProtectiveStopMarket`) and trigger/quantity modification (`ModifyProtectiveStop`) with automatic market protection. Request encoding matches Kite Connect's documented `trigger_price` and `order_type=SL-M` parameters. The native mock broker models protective stop acknowledgements and edits. These commands are **not** registered in the ILRC live runner or native order dispatcher; a broker receipt is not proof that an exchange stop is active. Stop-order rejection, trigger handling, reduce-only exposure, persisted identity, correct tick size, and fill-by-fill reconciliation must be confirmed before actual deployment. No live trading gates are enabled.

## Broker-observed SL-M mock regression

The `kite-adapter` mock now has a test that creates an ILRC-style `SL-M` order, reads its `TRIGGER PENDING` state and one-contract quantity back through the mock broker snapshot, then modifies the trigger and confirms the changed value from a fresh snapshot. A missing stop-order ID is explicitly rejected. No real Kite API or Redis call is made by this test. These checks do not replace required real-time fill/stop coordination, production Redis recovery, tick-size handling, or broker-confirmed live protection.

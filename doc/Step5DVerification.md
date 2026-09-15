# Step 5D: native Nautilus execution report mapping

This checkpoint maps Redis simulation journals to the actual Nautilus 0.63.0
OrderStatusReport, FillReport and ExecutionMassStatus Rust types.
It does not start an execution engine, call Kite or enable live orders.

## Verify

```bash
cd /home/ubuntu/RustNautilasProject
cargo run --locked -p kite-node -- reports-sim
```

The command creates a fresh Redis simulation namespace. Expected result:

```json
{
  "event": "nautilus_reports_simulation_complete",
  "native_order_reports": 3,
  "native_fill_reports": 3,
  "unresolved_orders": 1,
  "oms_ack_not_venue_acceptance": true,
  "replay_reports_identical": true,
  "native_serialization_roundtrip": true,
  "reports_complete": false,
  "execution_engine_started": false,
  "broker_accessed": false,
  "live_orders_enabled": false
}
```

reports_complete=false is intentional. A simulation journal is not a complete
broker account snapshot and contains no authoritative account positions.

## Components

| Component | Source |
| --- | --- |
| Native simulation identity and stable report IDs | crates/kite-execution/src/reports/identity.rs |
| Order status mapping | crates/kite-execution/src/reports/order.rs |
| Fill mapping | crates/kite-execution/src/reports/fill.rs |
| Batch assembly and reconciliation issues | crates/kite-execution/src/reports/batch.rs |
| Verification scenario | crates/kite-execution/src/reports/verification.rs |
| CLI | apps/kite-node/src/reports_command.rs |

The Redis journal now exposes an immutable view of its validated records,
recording timestamps and generation. It keeps this history after creation,
append and reopen. No event storage format change or alternative database is
introduced. Failed writes do not publish unconfirmed history in memory.

## Status mapping and incomplete data

| Journal state | Native report behavior |
| --- | --- |
| Accepted (OMS acknowledgement only) | Submitted; does not claim venue acceptance |
| PartiallyFilled | PartiallyFilled |
| Filled | Filled |
| Cancelled | Canceled |
| Prepared, Dispatching, Unknown, Rejected without a broker ID | Omitted; flagged for reconciliation |
| Any unresolved management command | Order report omitted; flagged for reconciliation |

Known, validated fills remain in the batch even when an unresolved management
command blocks the order snapshot. No broker ID is invented for an unresolved
submission. Terminal management commands no longer block an order report.

No real exchange acceptance timestamp was recorded, so ts_accepted is zero
as an explicit unavailable value. Other native report timestamps are the
persisted local simulation timestamps, not exchange times. A backward journal
clock blocks report generation instead of silently rearranging events.

The batch carries simulation_local_timestamps, simulation_zero_commission,
account_positions_not_reported and venue_acceptance_time_unavailable issue
markers. It also marks omitted order snapshots with order_requires_reconciliation.
A journal handle with an uncertain write outcome cannot export reports until
it has been reopened and reviewed.

## Prices, quantities, fees and products

Quantities remain contracts. Prices use exact decimal conversion from integer
paise. Average fill price is the quantity-weighted average calculated with
rust_decimal; no binary floating-point price conversion is used.

FillReport requires a commission. The simulator supplies an explicitly synthetic
zero INR commission and NoLiquiditySide, because no broker fee/liquidity data
was supplied. These values must not be treated as actual brokerage or used for
production P&L. Account ID is the fixed synthetic KITE-SIM.

MIS/NRML membership is retained in a separate product_by_client map because the
native order report type has no Kite product field. This is not yet production
product-aware position mapping. No position report or zero position is fabricated.

The simulator uses repeated fixture client/broker/trade IDs across independent
runs. Never merge these separate run namespaces into one execution account.
Production identity and account/date scope remain pending.

## Stable reports and replay

Report IDs derive deterministically from journal generation, report kind and
event sequence using SHA-256 truncated to 128 bits and formatted for the native
UUID4 wrapper. These are deterministic UUID-shaped identifiers, not randomly
generated UUIDs. They are identifiers, not signatures or tamper protection.

The mass report uses the journal's latest recording timestamp as its simulation
initialization time. Building a report from an unchanged journal after reopening
produces the same report content and IDs. Exact duplicate fills do not append
another record or produce another fill report.

Native serde serialization and deserialization preserve the complete batch.
ExecutionMassStatus is always marked reports_complete=false with the observed
journal time window. Empty position reports cannot be interpreted as proof that
the account is flat.

## Verification

- All 104 workspace tests passed.
- One process-exit fixture remains ignored during normal discovery and is
  explicitly invoked by its parent test.
- Seven new tests cover native mapping, exact price/quantity/average handling,
  timestamps, unresolved management, unknown submissions, product metadata,
  duplicate fills, generation separation, serialization and uncertain writes.
- Reopened Redis journals generate identical native reports.
- Corrupted timestamp ordering fails for review.
- Clippy with warnings denied passed.
- CLI verification against development Redis returned three order reports,
  three fill reports and one unresolved order with all expected flags.
- Formatting and Git whitespace checks passed.

Implementation signatures were checked against the installed, pinned
nautilus-model 0.63.0 source files under src/reports/.

## Remaining work

This is the native report-mapping boundary, not a Nautilus ExecutionClient or
engine integration. Production broker observation capture, account/date identity,
exchange timestamps, actual fees, position reports, ambiguity reconciliation and
real place/modify/cancel transport remain pending. Strategy logic and risk
controls also remain pending.

User verification passed; committed and pushed as dcabeee.

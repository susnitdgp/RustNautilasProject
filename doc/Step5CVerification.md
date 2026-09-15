# Step 5C: mock modification and cancellation with Redis recovery

Step 5B was verified by the user, committed as 4d2c75f and pushed to origin/main.
This checkpoint adds mock modification/cancellation, not live Kite execution.

## Manual verification

```bash
cd /home/ubuntu/RustNautilasProject
cargo run --locked -p kite-node -- order-management-sim
```

Each invocation uses a fresh Redis namespace. Expected result:

```json
{
  "event": "order_management_simulation_complete",
  "mock_calls": 5,
  "modification_confirmed": true,
  "cancel_ack_kept_order_open": true,
  "cancellation_confirmed": true,
  "partial_fill_preserved": true,
  "ambiguous_cancel_retry_blocked_after_restart": true,
  "broker_accessed": false,
  "live_orders_enabled": false
}
```

There are two synthetic placements, one modification and two cancellation
attempts. The second cancellation has a simulated ambiguous timeout.
The journal is reopened and a second attempt is blocked without a mock call.
The synthetic price is not a market quote or recommendation.

## Separate components

| Component | Path |
| --- | --- |
| Management events and lifecycle | crates/kite-journal/src/actions.rs |
| Modification request translation | crates/kite-execution/src/management/modify.rs |
| Cancellation request translation | crates/kite-execution/src/management/cancel.rs |
| Persist-before-send coordinator | crates/kite-execution/src/management/coordinator.rs |
| Verification scenario | crates/kite-execution/src/management/verification.rs |
| CLI | apps/kite-node/src/management_command.rs |

The existing Redis event journal, AOF acknowledgement and shared order-request
budget are reused. No SQLite or alternative state persistence is introduced.
Every management operation is attached to a locally owned order and command ID.

## Lifecycle

Prepare records the requested change without altering current order terms.
Only confirmed open or partially filled orders with an established broker ID
can be managed. Only one unresolved command per order is allowed.

The coordinator validates the command, reserves shared rate capacity, then
persists Dispatching before calling the concrete MockBroker. A budget deferral
leaves the command Prepared and does not consume a modification attempt.
A lost response or process interruption after dispatch does not permit retry.

Acknowledged means the mock request was acknowledged, not that the order was
modified or cancelled. A separate synthetic confirmation updates terms or
cancellation state. Rejected commands retain the original order terms.
An ambiguous response records Unknown and requires confirmation/reconciliation.

Exact repeated confirmations are ignored. Conflicting broker IDs, duplicate
command IDs, early confirmations and invalid state transitions fail closed.
A late acknowledgement after confirmation cannot regress state.

## Modification rules

Regular LIMIT/DAY only. Quantity is total order quantity in contracts, not an
increment and not barrels. Price is integer paise, aligned to the one-rupee tick,
then translated exactly to integer rupees.

The proposed total must exceed already filled quantity when prepared/sent.
A no-op change is rejected. Existing simulation bounds remain 1..100 contracts.
A maximum of 25 attempted modifications per order is persisted in Redis.
Dispatched modifications count even when rejected or uncertain; budget deferrals
do not. The 26th attempt is blocked after restart as well.

On confirmation the new quantity cannot be below recorded fills. Confirmation
against a terminal order fails for reconciliation instead of reopening it.
If fills reach the newly confirmed total while modification is pending, the
confirmed state becomes Filled.

## Cancellation and fill races

An acknowledgement alone leaves the order open. Confirmation preserves partial
fills and marks the remainder cancelled. A fill can win the cancellation race:
if the order is already Filled, a later cancellation confirmation never erases
the fill or changes the order back to open.

The existing fill checks and trade-ID deduplication remain in force. Delayed
fills can be recorded after cancellation up to the known total.

## Important implementation limits

This is a deterministic mock interface. Real HTTP PUT/DELETE calls, native
Nautilus execution reports, broker-history matching and account/date ownership
binding remain pending. The shared rate limiter budgets reservations; dispatch
timing guarantees still belong to the future real transport.

No exchange revision IDs or broker event ordering are modeled yet. A fill is
checked against the currently confirmed order terms. Fills that conflict with
those terms during a modification race fail for reconciliation rather than
guessing which revision executed. Production integration must durably capture
and reconcile such broker observations before proceeding.

There is no management-command abandonment flow yet: a Prepared command that
becomes invalid because the order filled must be reviewed, not blindly resent.
No strategy, risk engine or live order submission is enabled by this checkpoint.

## Verification

- All 97 workspace tests passed.
- One application process-exit fixture remains ignored for normal discovery and
  explicitly invoked by its parent test.
- Clippy with warnings denied passed.
- Nine new tests cover modification acknowledgement/confirmation, restart,
  partial fills, cancellation races, duplicate IDs, broker identity mismatch,
  rejection, translation, rate deferral and the 25-modification cap.
- The management restart test reopens a saved Dispatching command; the existing
  journal tests separately cover abrupt application exit and Redis process kill.
- CLI simulation against development Redis produced the expected five mock calls
  and all verification flags true, with broker_accessed=false.
- Formatting and Git whitespace checks passed.

Reference: [Kite order operations](https://kite.trade/docs/connect/v3/orders/)
and [Kite rate and modification limits](https://kite.trade/docs/connect/v3/exceptions/).

User verification passed; committed and pushed as 4e221b9. Next checkpoint: native Nautilus
execution reports and reconciliation boundaries, before production broker transport.
Strategy implementation remains pending.

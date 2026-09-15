# Step 4: read-only account and reconciliation verification

Step 3 was reviewed by the user (16 captured quotes and 16 replay callbacks),
committed as 0464122, and pushed to origin/main. Step 4 is left uncommitted
for review.

## Run on the dev server

```bash
cd /home/ubuntu/RustNautilasProject
cargo run --locked -p kite-node -- reconcile config/crudeoil-september.toml
```

Expect `event: reconciliation_complete`, `stable_observation: true`,
`consistency_checks_passed: true`, and an empty `issues` list.
Both `live_orders_enabled` and `execution_ready` remain false.
Compare target order/trade/position-bucket counts with Kite for the exact
CRUDEOIL26SEPFUT contract. A closed position may still have a zero-quantity
bucket. No test order is needed.

## Separate components

| Component | Source |
| --- | --- |
| Redis credential access | crates/kite-adapter/src/credentials/redis.rs |
| Fixed GET-only authenticated transport | crates/kite-adapter/src/http/authenticated.rs |
| Account identity and exchange validation | crates/kite-adapter/src/account/profile.rs |
| MIS / NRML mapping | crates/kite-adapter/src/account/products.rs |
| Order projection | crates/kite-adapter/src/orders/mod.rs |
| Trade projection | crates/kite-adapter/src/trades/mod.rs |
| Net and day position projections | crates/kite-adapter/src/positions/mod.rs |
| Target selection and sorted observations | crates/kite-adapter/src/reconciliation/snapshot.rs |
| Quantity and identity checks | crates/kite-adapter/src/reconciliation/checks.rs |
| Bounded sampling service | crates/kite-adapter/src/reconciliation/service.rs |
| CLI orchestration | apps/kite-node/src/reconciliation_command.rs |

Credentials are loaded once from Redis using susanta:kite_api_key and
susanta:kite_access_token. Profile, orders, trades and positions are GET-only.
Transport has connect/read deadlines, no redirects, an 8 MiB body limit,
sensitive authorization headers, and sanitized errors. Raw response buffers
are zeroized when released. Typed account data stays in process memory;
no raw account snapshot or credential is printed or written by this command.

## What is checked

The service takes two consecutive target observations, with a third observation
if the first pair changes. It compares sorted identity, product, status and
quantity fields, excluding live prices/P&L. An IST date rollover or continuing
change is reported as an issue. A 90-second overall deadline bounds the run.

For the selected instrument, checks cover identities, duplicate order/trade IDs,
known sides/statuses, product support/profile permission, order quantities,
trade-to-order linkage, and aggregate fills versus filled order quantities.
MIS and NRML remain distinct. Net positions are checked against overnight
quantity plus today's buys minus sells; today's product activity is compared
with trades. Day rows are checked for identity, product, duplicate buckets and
nonnegative activity, but are not independently rebuilt.

Orders are classified as external because this application has not submitted
orders and has no ownership journal yet. No external order is adopted, cancelled
or modified. Nonterminal orders and nonzero position buckets are counted
separately.

## Interpretation and limits

Matching samples are a stable observation of the selected fields, not an
atomic broker snapshot or proof that the account cannot change immediately
afterward. Product conversion can produce activity mismatches; this requires
review, not automatic correction. Prices, P&L, cash, margins, fees and holdings
are not reconciled. Profile product permission is not proof of contract-specific
order eligibility.

This stage checks broker data consistency. It does not yet construct Nautilus
execution/account reports, rebuild an execution engine, establish local order
ownership, or certify readiness to trade. Today's order/trade endpoints are
not historical ledgers. A failure exits nonzero after emitting safe issue codes
when a summary is available.

## Verified on 2026-09-15

- All 65 workspace tests passed, including nine new transport/reconciliation tests.
- Clippy with warnings denied passed.
- Live Redis-authenticated reconciliation returned a stable, consistent result.
- Target orders: 0; trades: 0; net position buckets: 0.
- MCX profile validation passed; profile permits MIS and NRML.
- No broker mutation endpoint was called.

Synthetic tests cover carried positions, duplicate fills, missing orders,
partial cancellations, product conversion mismatches, invalid identities,
unknown sides/statuses, arithmetic limits and private-data redaction.

## Official API references

- [User profile and products](https://kite.trade/docs/connect/v3/user/)
- [Orders and daily trades](https://kite.trade/docs/connect/v3/orders/)
- [Net and day positions](https://kite.trade/docs/connect/v3/portfolio/)

Next checkpoint after review: durable local order intent/ownership journal
and execution behavior tested against a mock broker, including ambiguous
timeouts and restart recovery. Live order submission remains disabled.

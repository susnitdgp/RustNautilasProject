# Step 5B: shared Redis order-request budgets

This checkpoint is an offline simulation prerequisite. Strategy logic and
real Kite order execution are not implemented. No live order transport exists.

## Verify

```bash
cd /home/ubuntu/RustNautilasProject
cargo run --locked -p kite-node -- rate-limit-sim
```

The command creates a fresh simulation namespace. Expect result fields:

- event: rate_limit_simulation_complete
- workers: 2, admitted: 1, deferred: 1
- restart_preserves_budget: true
- cooldown_shared: true
- deferred_intent_stays_prepared: true
- blocked_mock_calls: 0
- broker_accessed: false, live_orders_enabled: false

The diagnostic deliberately uses a one-request-per-day policy to make rejection
deterministic. It does not submit orders or contact Kite.
The existing execution-sim command also remains supported and now uses the limiter.

## Separate modules

All limiter modules are under crates/kite-execution/src/rate_limit/:

| Component | File |
| --- | --- |
| Policy and bounds | policy.rs |
| Redis connection and operation wrapper | store.rs |
| Atomic shared budget decisions | reserve.lua |
| CLI verification scenario | verification.rs |

The coordinator now requires a Limiter before any mock submission.
Redis connection and AOF acknowledgement utilities are reused from
kite-journal/src/connection.rs. No alternative state database is introduced.

## Policy and scope

Default reservation limits are 10/rolling second, 400/rolling minute and
5000/rolling 24 hours. A configured policy can lower but cannot exceed these
bounds. The 24-hour window is intentionally more conservative than a daily
reset. These defaults follow the published
[Kite order limits](https://kite.trade/docs/connect/v3/exceptions/), checked
on 2026-09-15. Broker limits can change and must be rechecked before live use.

A shared key uses:
susanta:nautilus:sim:order-budget:{ACCOUNT_SCOPE}

Every worker and instrument for the same simulated account must use the same
scope and policy. Production account identity binding is still pending.
A fresh UUID scope is used only to isolate standalone diagnostic runs; it
must not become a per-process account scope in production.

Creating an existing scope fails. Opening missing state or a mismatched policy
fails. State has no TTL and no automatic reset. The application never refunds
reservations after ambiguous outcomes, rejected requests, a journal failure or
a process exit. This can underuse capacity but prevents unsafe retries.

## Atomicity, persistence and cooldowns

Redis TIME supplies the clock. A Lua operation validates state and history,
computes all three rolling budgets, and writes the new reservation using a
single SET. All workers see the same account history. Entries older than
24 hours are pruned; at most 5000 timestamps are retained.

Allowed reservations and cooldown changes require local Redis AOF acknowledgement
via WAITAOF before returning. noeviction and the existing Redis 7.2+ / appendonly
requirements apply. No API key, token, account details or raw Redis errors are
printed. No automatic network retries occur.

A backward Redis clock, corrupt/missing state, TTL or Redis failure blocks the
operation. Failed operations poison that limiter handle until reopened.
AOF is local disk acknowledgement, not replicated durability or protection
against host loss.

Cooldowns are shared across workers and can only be extended, not shortened.
The mock broker now has a RateLimited outcome. It updates the shared cooldown
and leaves the attempted order Unknown, preventing blind retries. This models
429 handling; parsing actual HTTP responses and broker-specific Retry-After
behavior remain part of the future transport.

The coordinator checks that an intent is Prepared, reserves capacity, persists
Dispatching, then calls the mock. A budget deferral leaves the intent Prepared
and makes no mock call. Duplicate submission attempts are rejected before
consuming another reservation.

## Practical limits

This module limits reservation times. A real HTTP dispatcher must also bound
the delay between reservation and sending, avoid releasing delayed batches,
and handle transport deadlines. It is not yet a guarantee of request arrival
rate at the broker.

There is no automatic sleep/retry loop. The limiter exposes retry_after_ms for
a future bounded scheduler. Quote/history/other REST endpoint limits, the
25-modifications-per-order cap, cancel/modify transport and production account
coordination remain pending. Existing read-only commands are not routed through
this order-request limiter.

State validation and Lua history scans are bounded but not throughput-optimized.
Redis calls are synchronous and remain outside the Nautilus core event loop.

## Verification

All 88 workspace tests passed, including eight new rate-limit tests. One
child-process fixture is ignored by normal discovery and explicitly invoked
by its parent recovery test.

New tests cover:
- Twelve concurrent workers admitting exactly three reservations under a shared cap.
- Independent rolling-second, rolling-minute and rolling-day limits.
- Account isolation and policy mismatch.
- Shared cooldowns that cannot be shortened.
- Recovery after killing and restarting isolated Redis.
- Corrupt state, backward clock and poisoned handles.
- Deferred intents staying Prepared with no mock broker call.
- A mock 429 blocking another worker.

Clippy with warnings denied passed. Both rate-limit-sim and execution-sim
completed against development Redis with broker_accessed=false.
Tests use isolated Redis processes and synthetic data.

## Status and next work

Completed: instrument preflight, Redis credentials, market-data adapter,
quote capture/replay, read-only broker consistency checks, Redis simulation
journal, mock order lifecycle, and shared order-request budget simulation.

Pending: production account/date ownership, reconciliation of ambiguous
submissions, native Nautilus execution reports, real place/modify/cancel
translation and transport, live dispatch scheduling, strategy and risk controls.

Changes are left uncommitted for the user's manual verification.

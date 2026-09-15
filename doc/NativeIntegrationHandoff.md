# Native Nautilus integration handoff

Latest user-selected endpoint: http://94.136.191.37:3000/. Its current static mock responses fail native execution compatibility checks. Configuration and findings: [CustomSandbox.md](CustomSandbox.md). Real orders remain disabled.

Updated 15 September 2026. Repository: /home/ubuntu/RustNautilasProject, main.
Use git log -1 for the latest review checkpoint.

## Current outcome

Core native integration and the requested hardening are implemented and tested for
the supported one-contract LIMIT/DAY strategy. Long and short entries/exits are
explicit: BUY, BUY_EXIT, SELL and SELL_EXIT. Local stop-loss/target checks are added.
Read NativeHardening.md for architecture, limitations, commands and review guidance.
Real orders remain disabled. The user chose the official Kite sandbox for testing.
No webhook implementation is included. Manual code review precedes real trading.

## Verification

- Workspace: 173 passed, zero failed. One existing ignored child fixture is
  exercised by its passing parent crash test.
- Adapter tests with legacy live-orders feature enabled: 64 passed, zero failed.
  The real native factory still cannot dispatch orders.
- Clippy all-targets with -D warnings, rustfmt and diff checks pass.
- Native long and short mock runs: two fills each, flat at completion.
- Short full-packet replay through native BacktestEngine: two fills, flat.
- Native event histories and Redis command ownership reconstruct exactly.
- Account contention, Redis restart, budget retention, changed ownership,
  transient-read retries, task abort/join and SIGTERM shutdown are tested.
- Saved REAL Kite market-data catalog fc63b2f5-56e1-4fda-8d54-7914e8517c72:
  all 28 packets contain full fields, both timestamps and all five populated
  bid/ask levels. This is an audit of the saved capture, not a new live recording.
- Upstream vendor patches are unchanged from the previous verified checkpoint:
  1152 execution tests and 39 TWAP tests passed; one upstream ignored test.

## Strategy and execution locations

Strategy: apps/kite-node/src/native_node/strategy.rs.
Signal definitions/local protection: native_node/signals.rs.
Lifecycle/order validation: native_node/actor.rs.
Parameters: config/strategy-crossover.toml; enable_short=true, sample stop distance
30 rupees and target distance 60 rupees. These are price distances, not INR PnL.
Instrument: config/crudeoil-september.toml, standard CRUDEOIL26SEPFUT.MCX.
Runtime and backend selection: native_node/runner.rs.
Native Kite adapter modules: crates/kite-adapter/src/execution/native_client/.
Sandbox settings: config/kite-sandbox.toml. expected_user_id is deliberately a
placeholder until sandbox authentication succeeds and the account can be verified.

    cargo run --locked -p kite-node -- native-backtest config/strategy-crossover.toml
    cargo run --locked -p kite-node -- native-kite-mock config/strategy-crossover.toml
    cargo run --locked -p kite-node -- native-kite-mock-short config/strategy-crossover.toml
    cargo run --locked -p kite-node -- native-kite-status MOCK
    cargo run --locked -p kite-node -- native-kite-review NAMESPACE
    cargo run --locked -p kite-node -- native-recover NAMESPACE
    cargo run --locked -p kite-node -- native-full-audit CATALOG_PATH

## Official sandbox blocker

    cargo run --locked -p kite-node -- native-kite-sandbox-preflight
    cargo run --locked -p kite-node -- native-kite-sandbox config/kite-sandbox.toml config/strategy-crossover.toml

Both sandbox:kite_api_key and sandbox:kite_access_token exist in Redis. The supplied
values returned HTTP 403 on official sandbox read-only checks. No sandbox orders
were sent. Token presence does not prove server acceptance. Resolve sandbox access,
run preflight, pin its user_id in the sandbox config and verify MCX contract support.
Do not fall back to production credentials, routes or a different instrument.

Fixed REST root: https://sandbox.kite.trade/oms.
Fixed stream: wss://ws-sandbox.kite.trade, including verified user_id on reconnects.
Sandbox fees are explicitly estimated as zero because virtual contract notes are
unsupported there. Authenticated official sandbox fills remain UNVERIFIED.

## Operational limits and recovery

One durable account owner across participating native clients/processes. No TTL
lock takeover. Application budgets are 5/second, 100/minute and 1000/rolling day;
clean restart preserves budgets/cooldown. Unclean restart remains blocked for review.
Read snapshots have bounded transient retries. Mutations never retry automatically.
Polling/Redis failures stop admission, signal shutdown and retain recovery evidence.
Graceful shutdown releases ownership only when reconciled flat and error-free.
There is no automatic resume, command replay or unsafe forced-unlock command.

Stop-loss/target exits are local tick-triggered LIMIT orders, not exchange stops.
Gaps, disconnection and resting limits can prevent/delay exits. The next required
step is manual review and authenticated sandbox validation, not real-order activation.
Full tick data is five-level snapshots, not unique trades/aggressor-side/order-delta
market-by-order data. Native full replay retains complete payloads and timestamps.

## Cleanup and retained evidence

This hardening turn removed 18 further obsolete completed-simulation Redis keys;
154 other keys were preserved at cleanup, including all four credential keys,
order budgets and unfinished runs. Details: NativeHardeningCleanup.json.
Earlier cleanup removed 104 Redis keys and 91 old project logs: NativeCleanup.json.
This turn also removed three superseded hardening debug logs; final gate logs remain.
Do not remove the simulated exposure in cbf0a145-c68c-4b4b-a6b0-c8562689d60c;
it requires review and is not a real position. Preserve current verification data.

Current logs: /tmp/kite-hardening-workspace-tests.log,
/tmp/kite-hardening-feature-tests.log, /tmp/kite-hardening-clippy.log,
/tmp/kite-full-live-audit.log and /tmp/kite-sandbox-preflight.log.
Historical integration logs and catalog captures are retained as previously noted.

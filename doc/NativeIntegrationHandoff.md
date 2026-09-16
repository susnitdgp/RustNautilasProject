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

## Supertrend backtest addition (16 September 2026)

See [SupertrendBacktest.md](SupertrendBacktest.md). The separate native-supertrend-backtest
command uses Nautilus ATR(7), Wilder smoothing, multiplier 2 and five-minute bars.
The 15 September historical run completed with 174 session bars, 774 warmup bars,
16 simulated fills, 8 closed trades and INR 32,300 gross P&L, ending flat.
Reports and reproducible input are saved under backtest_results per run UUID.
Fees/spread/slippage are excluded. Real orders remain disabled.
The existing native-backtest command now saves its summary there too.

## VWAP / EMA / MACD seven-session backtest

See [VwapEmaMacdBacktest.md](VwapEmaMacdBacktest.md). Run native-vwap-backtest 2026-09-15.
Uses native session VWAP, EMA 9/21 cross, MACD 12/26/9 confirmation, ATR(14) Wilder
and a fixed 1.5 ATR simulated stop-market order. Entries use next-open prices;
all positions close daily. Includes September 14's evening-only holiday session.
The seven-session run (September 7–15) produced nine trades and INR 97,700 gross
P&L before fees/spread/slippage. All 1,122 requested bars validated; fills were audited.
Reports, per-day traces and offline replay input are retained in backtest_results.
Real broker orders remain disabled.

## Entry-filter comparison

See VwapFilterComparison.md. Native comparison on the exact saved seven-session input:
original 9 trades / INR 97,700 gross; zero-line + EMA slopes 2 / INR 12,500;
breakout confirmation alone 4 / INR 6,300. Both avoided the second September 15
loss but neither improved sample gross return. Original trades reproduced exactly.
The original entry remains default. Opposite-setup exits, one-lot sizing and 1.5 ATR
stops were held fixed. Reports are in backtest_results/vwap_filter_comparison_*.
New command: native-vwap-compare YYYY-MM-DD historical_input.json.
Real orders remain disabled. This is in-sample evidence only.

### Thirty-calendar-day VWAP comparison
Expanded comparison covers 2026-08-17 through 2026-09-15: 22 sessions / 3,732 bars.
All three entry variants remain unchanged, one September CRUDEOIL contract,
5-minute bars, fixed 1.5 ATR stop. Original evaluation candles preserved;
870 earlier Kite candles added only for warmup.
Original: 69 trades, INR 101,500 gross, INR 33,100 closed-trade drawdown.
Trend: 22 trades, INR -28,500 gross, INR 41,000 drawdown.
Breakout: 37 trades, INR 27,200 gross, INR 34,500 drawdown.
Original default unchanged; real orders remain disabled. Costs excluded.
All seven-session overlapping trades reproduce exactly in every variant.
See doc/VwapFilterComparison.md and the comparison directory ending 83cd24fa-b97f-4fec-a949-de59d3cb99d3.

### Supertrend + MACD + VWAP thirty-day comparison
See doc/SupertrendConfirmationReview.md.
Same 17 August–15 September input, five-minute candles and one lot.
Supertrend(7,2) plus MACD(12,26,9) and session VWAP entry confirmation:
143 trades (74 long, 69 short), INR 108,100 gross; closed-trade drawdown INR 46,200.
Supertrend alone: 191 trades, INR 83,900 gross, drawdown INR 64,600.
Exits remain Supertrend reversal or session close, with no extra fixed ATR stop.
Earlier EMA/VWAP baseline: 69 trades, INR 101,500 gross, drawdown INR 33,100;
different exits and trading frequency mean costs matter to any final choice.
No default changed; real orders disabled. Reports end in 2c404264-451e-4255-8a6a-20d0dbaad733.

### Fixed ATR stop comparison, 17 August–15 September
See doc/SupertrendStopComparison.md. Same confirmed Supertrend/MACD/VWAP strategy,
5-minute candles, one lot; fixed native 1.5 ATR(14) stop and fresh-direction
re-entry block carried across sessions through Redis.
Current: 143 trades, INR 108,100 gross, INR 46,200 closed-trade drawdown.
Stop variant: 143 trades, 70 stop-outs, INR 50,700 gross, INR 85,800 drawdown.
Worst trade improves from INR -14,900 to -12,500, but overall results worsen.
Baseline reproduces exactly. All stop prices and re-entry blocks audited.
197 workspace tests, Clippy and formatting passed. No default changed;
real orders disabled. Results directory ends cbc3cbb1-97fc-4165-91f9-c4b3f894922e.

### Ten-minute comparison without the added stop
Same 17 August–15 September 2026 period and one CRUDEOIL lot.
Supertrend(7,2) + MACD(12,26,9) + session VWAP, reversal/EOD exits only.
Ten-minute bars aggregate saved five-minute pairs: 1,866 evaluated bars,
435 prior warmup bars. No new historical fetch or stop orders.
5-minute: 143 trades, INR 108,100 gross, INR 46,200 closed-trade drawdown.
10-minute: 89 trades, INR 87,100 gross, INR 49,900 drawdown.
Worst loss: INR 14,900 versus INR 23,200; win rate 37.8% versus 42.7%.
Exact five-minute reproduction and all aggregation/entry/exit audits passed.
No default changed; real orders disabled. See doc/SupertrendIntervalComparison.md.
Results directory ends 19a53ea9-e08f-4868-815e-1d31516048fa.

### Production deployment preparation: selected five-minute strategy
User selected Supertrend(7,2) + MACD(12,26,9) + session VWAP, one lot,
five-minute candles, no added ATR stop. Pinned in config/production-supertrend.json.
Do not run native-node-paper as this strategy: runner.rs still wires the older
tick crossover. The selected BarStrategy is currently backtest-only.
native-production-preflight validates the selection and explicitly rejects live
activation with its known blockers. native-production-verify exercises the
selected strategy offline with the supplied release binary.
See doc/ProductionDeployment.md and deploy/{prepare_candidate,verify_candidate}.py.
Candidate packaging does not install/start a trading service or enable orders.
Live bars/warmup, gap/reconnect recovery, supported order policy, selected-strategy
forward testing and manual review remain required. Real orders remain disabled.

Release candidate prepared and verified on 16 September 2026:
    /home/ubuntu/kite-deploy/releases/20260916T072342Z-0f9f20e
Optimized build completed in 15m37s; 200 workspace tests passed plus Clippy/format.
Copied binary reproduces September 15 exactly: 6 trades, 12 fills, INR 21,800
gross, flat, no stop orders. manifest.json records source/binary/config hashes.
Source includes uncommitted changes; archive captures them explicitly.
Status is CANDIDATE_NOT_ACTIVATED. No live trading service installed or started.
Production remains incomplete until the selected strategy is wired to LiveNode,
forward-verified and reviewed. Do not confuse this candidate with live deployment.

## September 16: selected strategy connected to LiveNode

See doc/SupertrendLiveNode.md. New native-supertrend-sim and native-supertrend-paper commands run the selected five-minute Supertrend(7,2) + MACD(12,26,9) + session VWAP BarStrategy through LiveNode and Sandbox. One lot; no added ATR stop; real orders disabled. Paper duration 5–290 seconds.

Completed Kite historical bars warm up indicators and poll forward; WebSocket quotes provide simulated execution. Entries require fresh quotes and the latest completed bar after startup. Warmup gaps, revisions and feed faults stop admission. Hosted-mode SIGTERM drains a reducing exit before stopping; unresolved state retains a Redis owner for review.

Verification: 205 tests across workspace/final package runs, one existing ignored fixture; Clippy/fmt. Both-direction simulation and in-position SIGTERM regression pass. A clean short real-data run received 29 quotes. Longer real-data run 5761e7df-90b8-4a67-a28a-d2d213ec0ed4 processed one new bar and two simulated fills, then detected a revised completed candle and stopped flat. Its paired fills and final native cache were reviewed; the paper lock was released while retaining all fault/health reports. No service was activated, no real orders sent. Continuous unattended operation remains unqualified; historical revisions need an explicit recovery policy before enabling that scope.

Current code is newer than the inactive release candidate. Use cargo run for this connection. Do not use the old native-node-paper command for Supertrend; it still runs the tick crossover example.

## September 16: session operation and protected market orders

Current guide: doc/ProductionSession.md. Selected Supertrend + MACD + VWAP now submits MARKET/DAY; Kite wire translation sends market_protection=-1 without a limit price. Converted protected limit acknowledgements are reconciled using durable ownership and actual trades. Default production JSON and build keep broker mutations disabled. Production factory verifies the exact account, rejects pre-existing exposure/open orders and uses account-wide ownership/rate limits.

Added native-supertrend-session-paper, native-supertrend-kite-mock, native-supertrend-recovery-sim and gated native-supertrend-kite-production. Session stop begins 60 seconds before close. Corrected historical bars rebuild indicators after coverage checks; quote gaps pause decisions until data recovery and a fresh quote. Past orders are never replayed. Read retries are bounded. Redis owner monitoring stops on lost ownership. Fill deadlines produce controlled cancellation/review, never blind repricing. Unclean process restarts stay blocked for manual reconstruction/reconciliation.

Verification: 213 tests covered across workspace/final focused tests, one existing ignored fixture; 109 feature-enabled adapter tests. Native mock and SIGTERM/rebuild regressions pass. Real-data paper run 55cb0796-3c95-4071-80da-eb2fbb05af57: 360 seconds, 332 quotes, one new bar, one correction rebuild, two simulated fills and clean flat shutdown. No broker orders sent. Full-session qualification and manual/controlled broker validation remain. Source/terminal/service changes are not yet committed at this checkpoint.

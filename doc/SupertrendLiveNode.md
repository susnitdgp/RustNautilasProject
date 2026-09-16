# Supertrend + MACD + VWAP in LiveNode

Current session/recovery and protected-market broker wiring: see [ProductionSession.md](ProductionSession.md). Real execution remains disabled in the delivered configuration.

The selected strategy now runs in native Nautilus LiveNode with **Sandbox execution only**.
Selection: five-minute candles, Supertrend(7,2) using native Wilder ATR, native MACD(12,26,9) components and session HLC3 VWAP, one CRUDEOIL lot, no added ATR stop. Reversal, session-end and graceful-shutdown exits reduce the position.

Run from /home/ubuntu/RustNautilasProject:

```bash
cargo run --locked -p kite-node -- native-supertrend-sim
cargo run --locked -p kite-node -- native-supertrend-paper config/production-supertrend.json 290
```

The paper command runs for 5–86360 seconds during the configured session. It uses real Kite market data and simulated orders. It does not install or start a background service.

| File under apps/kite-node/src/native_node | Responsibility |
|---|---|
| supertrend_actor.rs | Shared BacktestNode/LiveNode strategy, signals and native orders |
| supertrend_live_runner.rs | LiveNode, Redis cache, Sandbox, bounded shutdown and reports |
| supertrend_live_data.rs | Native external-bar client; historical warmup and polling |
| supertrend_live_bars.rs | Completed bars, warmup continuity, gap/revision rejection |
| supertrend_live_control.rs | Freshness and stop state |
| supertrend_live_lease.rs | Durable single paper owner and final health record |
| data.rs | Existing Kite WebSocket quote/full-packet client |

Warmup and ongoing indicators use completed Kite historical candles. The client polls every ten seconds and excludes candles until two seconds after their close. It pauses on missing history and rebuilds indicators after validated OHLCV revisions. Quotes must be at most five seconds old. Entries wait for a newly completed bar after startup; warmup never places orders.

Execution uses fresh WebSocket quotes; Sandbox bar execution is disabled. Because historical candles arrive after close, paper fills occur after delivery, not necessarily at the next candle's open assumed by the backtest. This is a forward-test path, not proof of identical fills or profitability.

Shutdown uses LiveNode hosted mode: the host handles SIGTERM/Ctrl-C, blocks new entries, allows up to five seconds to flatten on fresh quotes, then stops the node. Missing fresh quotes, unresolved orders/positions or feed faults produce ReviewRequired. No stale-price forced fill is fabricated. Recoverable gaps/read failures pause admission and trigger bounded historical recovery. Exhausted recovery stops the run; automatic crash resumption is not enabled.

Redis keeps native orders, positions and saved strategy state in the run UUID namespace. A durable paper-owner key blocks another run until clean shutdown. Failed/crashed runs retain ownership for review. Do not delete an owner blindly; inspect the run's native cache and health record first. No automatic adoption of an earlier position occurs.

Diagnostic JSON is under data/supertrend-live/<UUID>/ (summary, indicators, signals and fills). This is a paper-run report; historical backtest results remain under backtest_results/.

The September contract and explicit session calendar are valid through September 21, 2026 only. Use native-supertrend-session-paper for the remaining market session. Real broker activation and automatic crash recovery remain separate. The old release candidate predates this integration; use cargo run to build the current source.

## Initial verification on September 16, 2026 (before revision recovery)

- Workspace checks: 205 tests across the workspace and final kite-node regressions; one existing ignored fixture. Clippy and formatting checked.
- Accelerated LiveNode fixture: 120 warmup + 40 five-minute bars, six simulated fills, both directions, flat.
- SIGTERM during a position: four simulated fills, reducing shutdown exit, flat; regression test added.
- 30-second real-feed run e53d74f7-698b-417d-b812-3ce9e3c68083: 826 warmup bars, 29 quotes, clean and flat, no new candle or trade.
- Longer real-feed run 5761e7df-90b8-4a67-a28a-d2d213ec0ed4: 828 warmup + one new bar, 168 quotes, BUY 1 at 10031 and shutdown SELL 1 at 10035, flat. Kite then revised a previously completed candle; the guard triggered ReviewRequired and the node flattened. This is evidence of fault handling, not a clean full-session qualification.
- A failed early SIGTERM fixture remains recorded for audit; the hosted-mode fix addresses that race. A zero-quote duration-validation failure was reviewed and its owner released after correcting the allowed duration.

Completed historical candles can be revised by the source. The initial implementation stopped on revisions. The current implementation rebuilds indicators from checked history without replaying prior orders. Extended unattended paper operation therefore remains unqualified. All observed fills above were generated by Nautilus Sandbox; no Kite broker order was submitted.

## Terminal status

Both Supertrend LiveNode commands now print a human-readable status snapshot every five seconds to stderr. No extra flag is needed. Startup steps, feed mode, simulated execution, five-minute interval and one-lot scope appear first. Snapshots show IST time, elapsed/remaining seconds, bid/ask, quote age, cached position, open/inflight order count, latest completed bar, bar/quote totals, rejected quotes, last signal and fill count. Status distinguishes connecting, warmup, waiting for fresh quotes or a completed bar, monitoring, draining, stopped and review required. Fault reasons and the final report directory remain visible.

stdout retains its JSON events. To keep the status visible while saving JSON:

```bash
cargo run --locked -p kite-node -- native-supertrend-paper config/production-supertrend.json 290 > paper-events.jsonl
```

The status belongs to that foreground process, not a background monitoring service. The synthetic command labels its feed SYNTHETIC; its accelerated historical bar timestamps differ from wall-clock quote timestamps. The display makes no claim of real broker execution or actual brokerage/P&L.

# Production deployment candidate: selected five-minute strategy

## Current deployment status

NOT ACTIVATED. The optimized candidate is for review and offline verification.
No selected-strategy live trading service is installed or started by this process.
Real orders remain disabled. No production or sandbox order test is implied.

The user selected Supertrend(7,2) + MACD(12,26,9) + session VWAP, five-minute
CRUDEOIL candles, one lot, no additional fixed ATR stop. Exits are Supertrend
reversal or session close. config/production-supertrend.json pins that selection.
The previous stop-loss and ten-minute experiments remain separate review code.

## Current LiveNode connection and remaining activation work

The selected BarStrategy now runs in LiveNode through supertrend_live_runner.rs.
See [SupertrendLiveNode.md](SupertrendLiveNode.md) for the new bounded paper commands,
completed historical-bar warmup, live quote execution and graceful shutdown.
The older native-node-paper command still runs the tick-based crossover strategy.

Execution for the selected strategy is Nautilus Sandbox only. Its native market
orders are not connected to the real Kite adapter's LIMIT/DAY submission path.
Historical-bar polling also introduces delivery latency relative to the backtest's
next-open fills. Live paper results cannot establish identical execution.

Remaining before unattended real trading: continuous session operation, controlled
reconnect/backfill and restart reconciliation, broker-compatible execution policy,
partial/unfilled exit handling and separately reviewed broker validation.
Faulted paper runs retain Redis ownership and require review; they do not resume
or silently adopt an earlier position. Real order submission remains disabled.

The previously packaged optimized candidate predates this LiveNode connection.
Use cargo run for current paper tests; it is not an activated production service.

## Build and validate

From /home/ubuntu/RustNautilasProject:

    cargo test --locked --workspace
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo fmt --all -- --check
    cargo build --locked --release -p kite-node -j 3

Production selection/readiness check:

    target/release/kite-node native-production-preflight config/production-supertrend.json

Expected: selection_valid=true, ready_for_live_deployment=false, nonzero exit
with explicit blockers. This is not a successful trading-service health check.
The configuration rejects live_orders_enabled=true, an ATR overlay, two lots or ten-minute bars.

Offline release verification (OUTPUT_DIR must not exist):

    target/release/kite-node native-production-verify config/production-supertrend.json 2026-09-15 backtest_results/supertrend_macd_vwap_2026-08-17_to_2026-09-15_2c404264-451e-4255-8a6a-20d0dbaad733/historical_input.json OUTPUT_DIR

The saved multi-day input must first be truncated through the requested day
(which is already true for September 15 above). Validation rejects future bars.
Expected September 15: 6 trades, 12 fills, INR 21,800 gross, flat.
It uses native BacktestNode and Redis persistence, never a broker executor.

## Candidate packaging

    python3 deploy/prepare_candidate.py

Creates /home/ubuntu/kite-deploy/releases/UTC_TIMESTAMP-GIT_HEAD/ with:
- bin/kite-node and config/production-supertrend.json.
- Review/deployment docs, source.tar.gz and source-status.txt.
- source-changes.patch for tracked edits (archive includes untracked source).
- manifest.json with binary/config/source SHA-256 hashes and inactive status.

The candidate records uncommitted source explicitly; Git HEAD alone does not
identify this build until the pending changes are committed. No credentials,
runtime captures or backtest reports are copied because they are Git-ignored.
The packaged configuration contains no credentials.

No current symlink is switched, no systemd trading unit is installed and no
autostart is enabled. Retain any existing runtime/recovery records.
Once live integration and forward verification are complete, write a separate
activation procedure with an exact immutable source revision, service identity,
credential source, readiness checks and rollback to the preceding release.
Do not activate this backtest-only candidate as an unattended trader.


After packaging, verify the exact copied binary:

    python3 deploy/verify_candidate.py /home/ubuntu/kite-deploy/releases/CANDIDATE_DIRECTORY

This records the expected readiness rejection and a reference-matched offline
September 15 replay in manifest.json. It is not live feed or broker validation.
The selected contract is explicitly September 2026, expiring September 21;
no automatic rollover is implemented. Contract selection must be reviewed before
any deployment beyond that expiry.

Source verification checkpoint: 200 workspace tests passed, zero failed,
one existing ignored fixture. Clippy with warnings denied, formatting and
diff checks passed. The production-selection test rejects real-order activation,
the discarded fixed stop, ten-minute candles and a changed position size.

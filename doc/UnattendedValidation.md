# Manual-run unattended validation — 16 September 2026

## Verdict
Failure handling passed the tests below. Unattended real-money trading is NOT yet qualified.
Launch mode is manual terminal execution, as requested. No Ubuntu service installation is required or attempted.
Real orders remain disabled. No real orders or Slack messages were sent during this validation.

## Verified
- Workspace before Slack changes: 216 passed, zero failed, one existing ignored fixture.
- Feature-enabled adapter: 110 passed, zero failed. Enabling the build feature in tests did not enable broker orders.
- New selected-strategy process tests: Redis ownership replacement stops with ReviewRequired and preserves the replacement; Redis outage stops unsuccessfully without claiming Clean; SIGKILL after a command attempt preserves account ownership and blocks restart without another command.
- Existing regressions: graceful SIGTERM flattening, both trade directions, native protected-market dispatch, Redis reconstruction, corrected-history rebuild without replaying orders.
- Existing adapter tests: lost acknowledgement without resubmission, persistence failure before dispatch, account rate budgets, bounded broker retries, stale-owner rejection and Redis restart.
- Clippy with warnings denied and formatting passed before Slack changes; see final verification addendum.
- Redis: AOF enabled, appendfsync everysec, noeviction, last write/rewrite status OK. Journal dispatch uses WAITAOF confirmation.
- Host: time synchronized, 37 GB free at inspection. This is a point-in-time check, not continuous monitoring.
- Existing optimized release replay passed twice with six trades, twelve fills, flat ending position and the saved reference result. Production preflight stayed blocked.
- Fixed release verification reruns: each invocation now creates a separate evidence directory instead of failing on an existing report folder.

## Evidence
- /tmp/kite-unattended-workspace.log
- /tmp/kite-unattended-feature.log
- /tmp/kite-unattended-clippy.log
- Release: /home/ubuntu/kite-deploy/releases/20260916T092521Z-d942472
- Separate verification_attempt_* directories within that release preserve each replay.
- Process regression source: apps/kite-node/tests/native_components.rs, module unattended.

## Remaining limits
A full-session forward run has not been completed. The previous real-data paper run lasted six minutes; this validation did not repeat it.
Actual Kite fills, market-protection conversion, partial fills and account reconciliation still need controlled broker validation.
Redis failure may prevent both the final Redis health write and summary-file creation; a nonzero process exit requires review even when a summary is missing.
An in-process Slack worker cannot alert after SIGKILL, machine failure or loss of all network connectivity. Independent heartbeat monitoring and an operator response procedure remain necessary for unattended operation.
No automatic crash takeover, daily restart or contract rollover is enabled. The current contract/calendar ends September 21, 2026.
Never clear owner records just to restart; inspect Redis recovery and broker state first.

## Manual paper run
From the repository directory, build the ordinary release and run:
```bash
cargo build --locked --release -p kite-node -j 3
./target/release/kite-node native-supertrend-session-paper config/production-supertrend.json
```
Start within the supported session. Keep the terminal attached and use Ctrl-C for graceful shutdown.
A surviving tmux session can keep a manually launched process alive across SSH disconnects if tmux is installed.
Session mode stops before market close and does not start itself tomorrow.
See SlackAlerts.md for optional notifications. Slack does not change order-enable gates.


## Slack integration verification addendum
After adding the optional notifier, both new Slack tests passed against loopback HTTP only. All nine native integration tests passed again, including the three process-fault cases. Workspace Clippy with warnings denied, formatting and diff checks passed. The existing workspace/feature counts above are from the preceding validation run, not a claim that the entire workspace was rerun after the notifier change. No external Slack delivery has been tested. The notifier is disabled by default and is not an independent crash watchdog.

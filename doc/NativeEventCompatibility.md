# Native event compatibility fixes

15 September 2026. These changes follow ab3ab01. Use git log -1 for the current checkpoint.
They address the warnings from the user's native-backtest output and the native
Sandbox immediate LIMIT submission path. Real broker dispatch remains disabled.
No production hardening work or live-order execution was performed.

## Startup hook

Nautilus 0.63.0 TwapAlgorithm forwards several DataActor hooks to ExecutionAlgorithm,
but omitted on_start. The native actor default logs the warning observed during
backtesting even though the user strategy and AuditActor override their hooks.
The local nautilus-trading patch forwards this hook to ExecutionAlgorithm::on_start.
This preserves TWAP lifecycle behavior and eliminates the default-handler warning.

## Immediate LIMIT event ordering

The native Sandbox queues Submitted, Accepted and Filled events for the LiveNode
runner. Previously process_limit_order accepted its local order snapshot, then
replaced the canonical cached order with that snapshot before the runner consumed
its queued events. Cache replacement also persisted Accepted. Subsequent Submitted
and Accepted processing therefore saw an advanced order state and emitted warnings;
Redis history required an application duplicate-event filter to reconstruct.

The local nautilus-execution patch retains lifecycle changes in the local snapshot
while creating the immediate fill. Existing canonical cache entries receive only
non-event liquidity-side metadata. The queued events then update canonical state
and native Redis in order. It neither rewinds cached orders nor drops lifecycle
events. Missing cache entries retain the upstream insertion behavior.

The application Redis adapter now delegates directly to Nautilus. Its previous
serialized-event HashSet and duplicate notification filter have been removed.
`native-recover` exposes order_event_sequences for review. The regression test
asserts exactly Initialized, Submitted, Accepted, Filled for each synthetic entry
and exit and rejects InvalidStateTrigger or missing-on_start warnings in subprocess
output. Emulator and TWAP namespaces are also reconstructed from Redis.

## Packaging and verification

Two published 0.63.0 crates are copied under vendor, preserving upstream license,
source, tests and manifests. Root patch.crates-io entries select the same copies
throughout the dependency graph. The Cargo registry was not edited. Exact patches
and source commit provenance are in vendor/README.md and the adjacent .patch files.
No remaining Nautilus dependency versions were upgraded.

Project workspace: 133 passed, 0 failed. Its ignored child-process fixture executes
through its parent crash test. Upstream nautilus-execution: 1,152 passed, 0 failed,
one existing ignored trailing-stop-market test. Upstream TWAP: 39 passed, 0 failed.
The ignored upstream test is not claimed verified. Clippy --all-targets with
-D warnings, cargo fmt --check and git diff --check passed.

The user's native-backtest command now completes with no startup warning and the
same 15 ticks, two fills, flat ending position and INR -400 simulated PnL.
Native-node-sim also completes without warnings, with two fills and flat ending
position. Its 15 complete packets pass native catalog readback. Real order gates
were not changed.

Logs on the development host:
- /tmp/kite-native-patched-workspace-tests.log
- /tmp/kite-native-patched-clippy.log
- /tmp/kite-native-event-sequence.log
- /tmp/kite-upstream-execution-tests.log
- /tmp/kite-upstream-twap-tests.log
- /tmp/kite-native-patched-backtest.log
- /tmp/kite-native-patched-sim.log

## Remaining integration scope

The later NativeKiteExecution.md checkpoint adds native order translation, an owned
broker event mapper and a read-only native client/factory. Redis-owned mock dispatch,
delayed-update polling and calculated-fee mass reconciliation are now integrated. These
compatibility patches do not provide a live enable flag. Changes to order
types beyond the currently used LIMIT/DAY path need corresponding native live-node
regression coverage; the patched upstream test suite is not proof of every live
Sandbox combination. Keep real orders disabled while completing the adapter.

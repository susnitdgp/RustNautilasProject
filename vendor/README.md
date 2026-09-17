# Nautilus 0.63.0 integration patches

Published crates copied from the Cargo registry, preserving upstream license,
source, tests and normalized manifests. Cargo.toml pins these exact local copies
through patch.crates-io so every dependency uses the same implementation.
No Cargo registry files were modified.

- nautilus-execution: immediate LIMIT matching fills from its local accepted
  snapshot without replacing canonical cached lifecycle state. This prevents
  Accepted persistence before Sandbox's queued Submitted/Accepted events.
  Only liquidity-side metadata is updated directly on an existing cached order.
- nautilus-trading: TWAP forwards DataActor::on_start to ExecutionAlgorithm::on_start,
  consistently with its existing lifecycle forwarding hooks.

These patches are intentionally narrow. The application's native node integration
suite rejects InvalidStateTrigger and missing on_start warnings and verifies
native Redis reconstruction. Re-evaluate these patches on any Nautilus upgrade.

Upstream source commit recorded in Cargo registry metadata:
`a0400251110653b6d8ae6a9b5b89c4543fa85a2d`.
Review the two adjacent .patch files to see the exact source differences without
reading the full vendored crates. Their versions stay 0.63.0; Cargo.lock removes
registry source/checksum entries for these two patched packages only.

Verification on 15 September 2026:
- Project workspace: 133 passing tests, including native queued event ordering,
  full catalog replay and Redis reconstruction without any event filtering.
- Upstream execution crate: 1,152 passing unit/integration tests; the upstream
  trailing-stop-market test remains marked ignored (not enabled or claimed passed).
- Upstream TWAP module: 39 passing tests.
- Clippy with warnings denied, formatting and diff checks pass for the project.

Upstream tests were run in an isolated temporary workspace containing these two
source copies, using the project target directory. Published crates omit one
include_str fixture; its original contents were downloaded from the exact source
commit above (test_data/databento/esh4-glbx-mdp3-20231225.mbo.json). No fixture was
fabricated and no upstream test expectations were changed. Historical regression
evidence remains available in earlier Git revisions.

# Step 1 results

Verified on vmi3506951 on 15 September 2026.

- cargo check --workspace: passed.
- cargo test --workspace: 18 integration tests passed; zero failures.
- cargo clippy --locked --workspace --all-targets -- -D warnings: passed.
- Compiled CLI with --download: passed against Kite public MCX master.
- Nautilus model dependency: 0.63.0; Rust: 1.98.0.
- Live result: CRUDEOIL26SEPFUT.MCX, token 144870151.
- Expiry: 2026-09-21; tick size: 1; broker lot_size: 1.
- Observation time: 2026-09-15T06:32:58.822010338Z.

No engine or WebSocket session was started. No account credentials were used.
No order APIs exist in this stage. Monetary multiplier remains unverified.
The Cargo.lock file pins the resolved dependency graph.

Manual check is ready: follow doc/Verification.md.
Next stage: separate authentication and WebSocket components, still data-only.

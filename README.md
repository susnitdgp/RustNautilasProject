# Rust Nautilus + Zerodha Kite

Rust workspace for a modular Kite integration. First target: standard MCX
CRUDEOIL September 2026 futures (not CRUDEOILM).

**Implemented:** modular preflight, Redis credentials, session validation,
WebSocket diagnostics, a Nautilus DataClient/factory and DataEngine quote runner,
plus Parquet recording, offline quote replay and read-only broker reconciliation. Nautilus is pinned to 0.63.0.
See [Step 3 verification](doc/Step3Verification.md) for capture/replay commands
and [Step 2 verification](doc/Step2Verification.md) for sample price display.
See [Step 4 verification](doc/Step4Verification.md) for the read-only reconcile command.
See [Step 5A verification](doc/Step5Verification.md) for the durable Redis journal
and offline mock execution simulator.
See [Step 5B verification](doc/Step5BVerification.md) for shared Redis order budgets.
See [Step 5C verification](doc/Step5CVerification.md) for mock modification and cancellation.
Full LiveNode trading, strategies, native execution reports and order submission
are not implemented yet.
See [implementation stages](doc/Implementation.md) and [verification](doc/Verification.md).

Run from this repository:

```bash
cargo run --locked -p kite-node -- preflight config/crudeoil-september.toml --download
cargo test --locked --workspace
```

Preflight loads the API key and access token from Redis before downloading the
public instrument master. See [Redis credentials](doc/RedisCredentials.md).
Configuration requires an explicit expiry. Expired targets fail; there is no
automatic roll. Token, lot size and tick size come from the downloaded master.

The reported broker lot size is NOT the monetary multiplier. The September
standard crude-oil contract now has a source-verified 100-barrel multiplier.
MIS/NRML profile mapping and broker quantity checks are implemented.
Margin, fees, native account reports and order handling remain later work.

Nautilus dependencies are LGPL-3.0-only; review upstream license obligations
before redistribution. This project consumes published crates rather than
modifying upstream engine sources.

# Rust Nautilus + Zerodha Kite

Rust workspace for a modular Kite integration. First target: standard MCX
CRUDEOIL September 2026 futures (not CRUDEOILM).

**Implemented:** modular instrument preflight, Redis credentials, session validation
and bounded WebSocket market-data diagnostics using Nautilus model 0.63.0.
See [Step 2 verification](doc/Step2Verification.md) for live data commands.
The Nautilus engine runtime, strategies and order APIs are not implemented yet.
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

The reported broker lot size is NOT a contract monetary multiplier. Contract
specifications, quantity semantics, margin and product mappings require separate
validation before orders can be implemented.

Nautilus dependencies are LGPL-3.0-only; review upstream license obligations
before redistribution. This project consumes published crates rather than
modifying upstream engine sources.

# Rust Nautilus + Zerodha Kite

Rust workspace for a modular Kite integration. First target: standard MCX
CRUDEOIL September 2026 futures (not CRUDEOILM).

**Step 1 only:** read-only instrument preflight using Nautilus model 0.63.0.
No WebSocket subscription, engine runtime, strategy or order API is implemented yet.
See [implementation stages](doc/Implementation.md) and [verification](doc/Verification.md).

Run from this repository:

```bash
cargo run --locked -p kite-node -- preflight config/crudeoil-september.toml --download
cargo test --locked --workspace
```

No API keys or login are needed for the public instrument master.
Configuration requires an explicit expiry. Expired targets fail; there is no
automatic roll. Token, lot size and tick size come from the downloaded master.

The reported broker lot size is NOT a contract monetary multiplier. Contract
specifications, quantity semantics, margin and product mappings require separate
validation before orders can be implemented.

Nautilus dependencies are LGPL-3.0-only; review upstream license obligations
before redistribution. This project consumes published crates rather than
modifying upstream engine sources.

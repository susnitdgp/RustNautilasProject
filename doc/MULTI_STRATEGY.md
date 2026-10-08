# Multi-asset strategy refactor (development foundation)

Configuration is JSON only. See `config/portfolio-development.example.json`.
Inspect without connecting to Kite or Redis:

```bash
cargo run --locked -p kite-node -- native-portfolio-validate config/portfolio-development.example.json
```

- One shared `broker_config` path identifies the account credentials; the portfolio validator never opens that file.
- Add up to four `instances`, each with an immutable `id`, strategy name, instrument, token and independent `strategy_config` JSON file.
- Multiple instances can use the same instrument token; subscription tokens are deduplicated before forming a shared-socket Kite subscribe/full-mode request.
- Redis names are `<redis_prefix>:v1:{<instance-id>}:<kind>`. Braces group instance keys in a Redis hash slot.
- `live_orders_enabled` MUST be false for every portfolio entry. **This initial foundation is non-executing.**
- Existing single-instrument ILRC live commands, private live credentials, and Redis journals are unchanged.

**Not implemented yet:** WebSocket multi-token frame routing into per-instrument candle aggregators, independently supervised Nautilus strategy actors, account-wide risk reservation and cross-strategy order coordination, per-instance real broker execution/reconciliation, and portfolio dashboard. Until these are implemented and tested, use the existing single-instrument runner only as separately authorized.

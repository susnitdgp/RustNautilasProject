# Multi-asset strategy refactor (development foundation)

Configuration is JSON only. See `config/portfolio-development.example.json`.
Inspect without connecting to Kite or Redis:

```bash
cargo run --locked -p kite-node -- native-portfolio-validate config/portfolio-development.example.json
```

- One shared `broker_config` path identifies the account credentials; the portfolio validator never opens that file.
- Add up to four `instances`, each with an immutable `id`, JSON `enabled` flag, strategy name, instrument, token and independent `strategy_config` JSON file.
- This example reserves NIFTY futures, CRUDEOIL futures, GOLD futures and BANKNIFTY futures. CRUDEOIL is enabled for *manifest inspection only*; the other three are disabled with explicit contract placeholders and token `0`.
- `enabled: false` instances do not enter the token subscription set. To enable one, first set the correct current contract symbol, verified token, strategy name and strategy JSON configuration, then switch `enabled: true`. Enabling an entry does not start trading or any background process.
- Multiple instances can use the same instrument token; subscription tokens are deduplicated before forming a shared-socket Kite subscribe/full-mode request.
- Redis names are `<redis_prefix>:v1:{<instance-id>}:<kind>`. Braces group instance keys in a Redis hash slot.
- `live_orders_enabled` MUST be false for every portfolio entry. **This initial foundation is non-executing.**
- Existing single-instrument ILRC live commands, private live credentials, and Redis journals are unchanged.

**Not implemented yet:** WebSocket multi-token frame routing into per-instrument candle aggregators, independently supervised Nautilus strategy actors, account-wide risk reservation and cross-strategy order coordination, per-instance real broker execution/reconciliation, and portfolio dashboard. Until these are implemented and tested, use the existing single-instrument runner only as separately authorized.

### Active research asset: standard CRUDEOIL October 2026

The development portfolio selects `CRUDEOIL26OCTFUT.MCX` (Kite token `145894407`) using ID `crudeoil26oct-ilrc` and existing `config/production-ilrc.json`. Its *planned* Redis keys are `kite-dev:v1:{crudeoil26oct-ilrc}:journal` and `kite-dev:v1:{crudeoil26oct-ilrc}:owner`. These are independent from the legacy execution journal/ownership keys; this configuration validator does not create or migrate any Redis keys or run a Nautilus strategy. All portfolio live order gates remain false. Contract changes/rollovers require a new verified token and deliberate handling of old state.

### Rollover-safe identity (development only)

The JSON `rollover` object separates permanent `strategy_id: crudeoil-ilrc` from month-specific `id: crudeoil-ilrc-202610`, `contract_month: 2026-10`, and verified current-contract expiry. Planned Redis keys use `kite-dev:v1:{crudeoil-ilrc-202610}:journal` and `kite-dev:v1:{crudeoil-ilrc-202610}:owner`. November has no configured token or expiry until verified against Kite; `next_contract` is therefore null. A proposed next contract cannot be approved/verified via the read-only portfolio skeleton. No Redis key is migrated, created, or deleted by validation, and the legacy ILRC runner remains isolated. Actual rollover requires broker-flat reconciliation, manual authorization and warmup on the new contract.

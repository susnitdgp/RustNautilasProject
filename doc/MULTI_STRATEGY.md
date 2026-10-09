# Multi-asset strategy skeleton

The JSON portfolio validator retains four independent instrument slots (NIFTY, CRUDEOIL, GOLD and BANKNIFTY). All are disabled until researched and approved. Market-data subscriptions and execution strategies are not wired to this read-only manifest.

A prospective strategy has a stable identity, and each futures contract month has a separate portfolio instance and Redis namespace. Old ownership and journal state must never be recycled between contracts. The current CRUDEOIL slot is `crudeoil-research-202610` with October instrument `CRUDEOIL26OCTFUT.MCX`, token `145894407` and `live_orders_enabled=false`; strategy is `unassigned`. No redis keys are created by validation.

## Selecting a strategy for a slot

`"strategy": "vce-mojo"` runs the Rust port of VCE-Mojo v1.6 (`crates/vce-mojo`) on that slot. The slot's
`strategy_config` points at its own JSON file (`config/vce-mojo-crudeoil.example.json`,
`config/vce-mojo-gold.example.json`) holding bar size, lots, point value, Pine inputs (including the EOD cut-off:
23:15 for MCX, set it to 15:15 for NFO slots) and backtest costs. `native-portfolio-validate` loads and checks
the file for every `vce-mojo` slot and fails if an enabled slot's file is missing or invalid.

Each slot gets its own Nautilus strategy ID (`VCE-<slot id>`), so several slots can run the same strategy on
different instruments with isolated positions. The Nautilus wrapper (`vce_strategy.rs`) is not yet wired to a
LiveNode runner, and `live_orders_enabled` remains rejected by the validator.

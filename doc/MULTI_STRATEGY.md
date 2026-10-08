# Multi-asset strategy skeleton

The JSON portfolio validator retains four independent instrument slots (NIFTY, CRUDEOIL, GOLD and BANKNIFTY). All are disabled until researched and approved. Market-data subscriptions and execution strategies are not wired to this read-only manifest.

A prospective strategy has a stable identity, and each futures contract month has a separate portfolio instance and Redis namespace. Old ownership and journal state must never be recycled between contracts. The current CRUDEOIL slot is `crudeoil-research-202610` with October instrument `CRUDEOIL26OCTFUT.MCX`, token `145894407` and `live_orders_enabled=false`; strategy is `unassigned`. No redis keys are created by validation.

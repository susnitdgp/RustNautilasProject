# Multi-asset strategy skeleton

The JSON portfolio validator retains four independent instrument slots (NIFTY, CRUDEOILM, GOLD and BANKNIFTY). All are disabled until researched and approved. Market-data subscriptions and execution strategies are not wired to this read-only manifest.

A prospective strategy has a stable identity, and each futures contract month has a separate portfolio instance and Redis namespace. Old ownership and journal state must never be recycled between contracts. The crude slot is `crudeoilm-sats-202610`: CRUDEOILM October mini futures `CRUDEOILM26OCTFUT.MCX`, token `145894663`, expiry 2026-10-19, lot size 1, `live_orders_enabled=false`; strategy `sats` with every input in `config/sats-crudeoilm.json`. Each slot's strategy settings live in their own JSON file referenced by `strategy_config`. No redis keys are created by validation.

# Active configuration

The active strategy is **MCX Crude PURE Squeeze Momentum v2.28.3**.

- `production-squeeze-momentum.json` — authoritative instrument, session, Pure SQZ strategy inputs and display settings.
- `candidate-squeeze-momentum-opt45.json` — optimization candidate; identical to production except SQZ transition is 45% instead of 70%. It is not the production config.
- `kite-production.json` — local/private Kite production settings.
- `kite-sandbox.toml` — local sandbox settings.

All strategy inputs are JSON-driven under `squeeze_momentum`. The committed candidate keeps `live_orders_enabled: false`.

Validate the candidate:

```bash
./deploy/verify-squeeze-momentum-v2283.sh
```

Record read-only Kite full ticks:

```bash
./deploy/record-squeeze-momentum-ticks.sh 3600
```

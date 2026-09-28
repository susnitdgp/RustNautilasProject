# Active configuration

The active strategy is **MCX Crude PURE Squeeze Momentum v2.28.3**.

- `production-squeeze-momentum.json` — authoritative instrument, session, Pure SQZ strategy inputs and display settings.
- `candidate-squeeze-momentum-opt45.json` — optimization candidate; identical to production except SQZ transition is 45% instead of 70%. It is not the production config.
- `candidate-squeeze-momentum-db5.json` — entry-deadband candidate; keeps the 70% exit and requires absolute SQZ momentum >= 5 for new entries. Wave reset remains unchanged.
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

`sqz_entry_deadband` is an entry-only absolute SQZ momentum filter. `0.0` preserves the v2.28.3 baseline exactly; positive values require BUY momentum >= +deadband and SHORT momentum <= -deadband. Wave reset remains sign/zero based.

The day-end square-off candle is entry-blocked: an existing position may exit on the candle closing 23:15, but a flat strategy cannot open a new BUY/SHORT on that candle.

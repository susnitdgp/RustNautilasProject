# Active configuration

The active strategy is **MCX Crude PURE Squeeze Momentum v2.28.3**.

- `production-squeeze-momentum.json` — authoritative active profile: Dynamic Wave DB EMA25×30%, fixed DB0, 2 weak bars, 50% transition exit.
- `candidate-squeeze-momentum-opt45.json` — retained comparison copy of the OPT45 / DB0 settings now promoted to production.
- `candidate-squeeze-momentum-db5.json` — entry-deadband candidate; keeps the 70% exit and requires absolute SQZ momentum >= 5 for new entries. Wave reset remains unchanged.
- `candidate-squeeze-momentum-dynwave25x30.json` — final-test dynamic wave candidate: OPT45 / fixed DB0 plus EMA25 of prior |SQZ| × 30%, frozen at each new sign-wave until zero cross.
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

Dynamic wave DB uses only prior confirmed bars for its EMA reference. The threshold is calculated once when a new positive/negative wave begins and remains frozen until the next zero/sign reset. A dynamic percentage of `0.0` disables the feature.

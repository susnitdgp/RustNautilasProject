# Active configuration

The active strategy is **MCX Crude PURE Squeeze Momentum v2.28.3**.

- `production-squeeze-momentum.json` — authoritative active profile: Dynamic Wave DB EMA25×30%, fixed DB0, 1 same-wave rebuild re-entry, 2 weak bars, 45% transition exit.
- `kite-production.json` — local/private Kite production settings.
- `kite-sandbox.example.toml` — sandbox configuration template.

The committed production strategy gate is enabled. `kite-production.json` is private/local and its broker gate must also be enabled for real order routing.

Validate the final production profile:

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

`same_wave_reentry_limit` controls how many same-direction rebuild entries are allowed after a QLX/QSX transition exit while SQZ remains on the same sign-wave. Production uses `1`. Re-entry still requires the frozen wave DB and normal strengthening bars; zero-cross resets the counter.

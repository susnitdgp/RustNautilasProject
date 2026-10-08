# Active configuration

The active strategy is **MCX Crude Smart Money Breakout Channels v1.7**.

- `production-smbc.json` — authoritative SMBC strategy profile and risk/trade parameters.
- `kite-production.json` — private/local Kite broker settings.
- `kite-production.example.json` — safe broker-settings template with live orders disabled.
- `kite-sandbox.example.toml` — sandbox configuration template.

The committed SMBC strategy gate is intentionally disabled while parity/paper verification is in progress.

Verify:

```bash
./deploy/verify-smbc-v17.sh
```

Read-only Kite full-tick recording:

```bash
./deploy/record-smbc-ticks.sh 3600
```

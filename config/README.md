# Active configuration

This directory contains only current operational configuration.

- `production-trend-ribbon.json` — authoritative Trend Ribbon v2.10 strategy,
  instrument, session calendar and realtime parameters.
- `kite-production.json` — local/private Kite production settings.
- `kite-sandbox.toml` — local sandbox settings.

The committed Trend Ribbon candidate keeps `live_orders_enabled: false`.
Do not change that gate merely to run tests or market-data recording.

Validate the current candidate with:

```bash
./deploy/verify-trend-ribbon-v210.sh
```

Record read-only Kite full ticks with:

```bash
./deploy/record-trend-ribbon-ticks.sh 3600
```

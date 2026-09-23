# Configuration layout

## Current Trend Ribbon production entry point

`deploy/run-trend-ribbon-live.sh` loads these two files at runtime:

| File | Purpose |
| --- | --- |
| `production-trend-ribbon.json` | Trend Ribbon parameters, candle interval, contract, calendar and strategy gate |
| `kite-production.json` | Private account settings and broker execution gate |

The `interval` field in the selected JSON controls the Trend Ribbon candle duration. The reviewed live selection currently uses `5minute`; `3minute` remains supported. The shared release binary is not built with a JSON file embedded as its live selection. Changing the folder layout does not start a strategy or select another one.

## Optional presets

Other JSON selections are retained unchanged in `backup/`. Optional launchers, source defaults and tests have been updated to use that location. These presets are not additional inputs to the Trend Ribbon live launcher.

The v2.0.0 source uses the relocated no-argument Supertrend simulation default. For archived strategies, pass the explicit `config/backup/...json` path or use the updated launcher. An older pre-migration executable may still name the former path until rebuilt. The release procedure rebuilds the executable; it does not launch it.

TOML configurations remain where they were for legacy, sandbox and test workflows. In particular, `kite-sandbox.toml` and the private edits in `kite-production.json` are not moved or rewritten.

## Safety

A file in `backup/` is not automatically disabled: its existing live-order flag is preserved. Do not launch another strategy merely to test these paths. Configuration checks do not establish broker readiness or fix the outstanding Trend Ribbon session-reset and quote-independent square-off findings in `doc/EngineeringDesignAndStrategies.md`.

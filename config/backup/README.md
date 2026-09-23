# Optional JSON configuration presets

These files were moved from the parent `config/` directory on 23 September 2026 to keep the active Trend Ribbon production inputs easy to identify. JSON contents were preserved byte-for-byte.

| File | Purpose |
| --- | --- |
| `pivot-point-supertrend.json` | Pivot paper/simulation selection |
| `production-pivot-supertrend.json` | Optional Pivot live selection |
| `production-supertrend.json` | Original Supertrend + MACD/VWAP selection |
| `trend-ribbon-boswaves.json` | Trend Ribbon paper/simulation selection |

This is an archive of optional presets, not a backup of private credentials and not an automatic disable switch. The existing production-capable preset retains its live-order flag. The Trend Ribbon live launcher does not read these files.

Use the explicit `config/backup/<filename>.json` path from the repository root when deliberately running an optional mode. The Pivot/Supertrend launcher scripts and the repository's tests use the relocated files. The legacy inactive-candidate packaging script reads its source preset here but keeps its own packaged `config/production-supertrend.json` layout.

No strategy logic, private configuration, production release executable or live session was changed by this relocation. Outstanding engineering findings remain documented separately.

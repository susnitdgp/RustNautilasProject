# Step 1 manual verification

Run on vmi3506951:

```bash
cd /root/RustNautilasProject
cargo test --locked --workspace
cargo run --locked -p kite-node -- preflight config/crudeoil-september.toml --download
```

Verify the downloaded output against the current broker instrument entry:

| Field | Expected from the 15 September 2026 observation |
| --- | --- |
| instrument_id | CRUDEOIL26SEPFUT.MCX |
| instrument_token | 144870151 |
| expiry | 2026-09-21 |
| tick_size | 1 |
| broker_lot_size | 1 |
| live_orders_enabled | false |
| contract_multiplier_verified | false |
| engine_started | false |

Token and broker lot size are observations, not immutable contract rules.
Compare the selected instrument with CRUDEOIL SEP FUT, not CRUDEOILM or an option.
Do not interpret this check as verification of a full Nautilus futures instrument.

Optional offline test with your own CSV:

```bash
cargo run --locked -p kite-node -- preflight config/crudeoil-september.toml --csv /absolute/path/mcx.csv
```

Offline input does not establish freshness. The output says so explicitly.
The command uses the host clock converted to Asia/Kolkata for expiry checks.
On expiry day, preflight may pass; trading-session cutoff enforcement belongs
to the later runtime and no trading is enabled here.

After checking, report: **Step 1 verified**, or paste the error and non-secret
instrument fields. Never paste API secrets, access tokens or login credentials.

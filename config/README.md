# ILRC production configurations

- `production-ilrc.json`: ILRC Combined 3-minute CRUDEOIL profile; live orders disabled.
- `kite-production.json`: local broker configuration; `live_orders_enabled` must remain `false`. Never commit secrets.
- `kite-production.example.json`: broker settings example with live orders disabled.
- `ilrc-live-integration.json`: fail-closed mock integration readiness policy, **not** an authorization for live trading.

Run `bash deploy/verify-ilrc-production.sh` for the manual shadow profile and `bash deploy/verify-ilrc-live-integration.sh` for mock readiness checks. See `doc/ILRCv1.md` for limitations.

# Project requirements

- No persistent trading state: order records, strategy state and the Nautilus cache live in memory for one run; Kite is the source of truth and there is no order journal or crash recovery. Redis holds only shared/operational data (Kite access token, shared order-rate budget, dashboard). Do not introduce SQLite or another state database.
- One process per slot and account, enforced by an OS lock file (`coordination::lock_instance`).
- Keep each component in a separate module.
- Keep live order submission disabled unless explicitly authorized by user

# Portfolio slots: features and limitations

The portfolio manifest (`config/portfolio-production.json`) lists the **slots** this project can
run. A slot is one strategy on one futures contract month, with its own settings file, its own
lock and its own logs. Code: `apps/kite-node/src/native_node/portfolio.rs`
(manifest and validation) and `crates/kite-adapter/src/execution/native_client/keys.rs`
(names).

Written for kite-node 2.21.0 / kite-adapter 0.4.0.

---

## 1. Current slots

| Slot ID | Strategy | Contract | Enabled | Live orders allowed |
|---|---|---|---|---|
| `crudeoilm-sniper-202610` | sniper (main) | CRUDEOILM26OCTFUT.MCX, token 145894663, expiry 2026-10-19 | yes | yes |
| `crudeoilm-sats-202610` | sats | CRUDEOILM26OCTFUT.MCX, token 145894663, expiry 2026-10-19 | no | yes |
| `nifty-fut` | unassigned (placeholder) | NIFTY_FUT_CONTRACT.NFO, token 0 | no | no |
| `gold-fut` | unassigned (placeholder) | GOLD_FUT_CONTRACT.MCX, token 0 | no | no |

---

## 2. Manifest fields

### 2.1 Top level

| Key | Value | Meaning |
|---|---|---|
| `version` | 1 | Only version 1 is accepted |
| `redis_prefix` | kite-prod | First segment of every Redis key the slots own |
| `broker_config` | config/kite-portfolio.json | Informational only: the runners take the broker settings file on the command line (`config/kite-production.json`). The file named here does not exist and is not read |
| `instances` | list | 1 to 4 slots |

### 2.2 Per slot

| Key | Meaning |
|---|---|
| `id` | Slot ID. With `rollover`, it must be exactly `<rollover.strategy_id>-<YYYYMM>` |
| `enabled` | Live runs refuse a disabled slot; backtests and paper runs still work on it |
| `strategy` | `sniper` or `sats`; anything else has no runner (`unassigned` = placeholder) |
| `instrument` | `<SYMBOL>.MCX` or `<SYMBOL>.NFO` |
| `instrument_token` | Kite token; required (non-zero) when enabled |
| `strategy_config` | Path to the strategy's own JSON (for example `config/sniper-crudeoilm.json`) |
| `live_orders_enabled` | Second gate for real orders (section 4) |
| `rollover.strategy_id` | Permanent strategy identity across contract months (for example `crudeoilm-sniper`) |
| `rollover.contract_month` | `YYYY-MM`; must match `expected_expiry` and the month in the symbol (`26OCT`) |
| `rollover.expected_expiry` | Runs are refused after this date; it must also match Kite's instrument master |
| `rollover.lot_size` | Contract lot size (CRUDEOILM 1) |
| `rollover.next_contract` | Placeholder for the next month; must stay unverified, unapproved, token 0 |

### 2.3 Validation rules (checked on every load)

* IDs, prefix and strategy identity: only letters, digits, `-` and `_`, at most 48 characters
  (Redis key safety).
* No duplicate slot IDs; no two slots with the same **instrument + strategy** pair.
  Different strategies on the same instrument are allowed (Sniper and SATS share CRUDEOILM).
* Enabled slots need a token, and one token may not map to two different instruments.
* `live_orders_enabled` is only accepted by a `--features live-orders` build.
* The slot's strategy config must agree with the slot: Sniper's config carries its own
  `symbol` and `instrument_token`, which must match the slot; SATS takes both from the slot.
  Both require product MIS.

Read-only check with the full key layout:
`./target/release/kite-node native-portfolio-validate config/portfolio-production.json`

---

## 3. What each slot gets (features)

### 3.1 Isolated state

Since kite-node 2.21.0 a run keeps its trading state **in memory only**: order records, the
Nautilus cache (orders, positions) and the strategy state all end with the process. Nothing is
journalled and nothing is reloaded; Kite is the source of truth. What remains outside the
process:

| State | Where |
|---|---|
| Instance lock (one process per slot and account) | file `~/.local/state/kite-node/locks/kite-prod-<slot>-<kite user>.lock` (`KITE_LOCK_DIR` overrides the directory) |
| Order budget (Kite rate limits) | Redis `kite-prod:v1:{account-<kite user>}:order-budget`, shared by all slots on the account |
| Kite access token | Redis, written by `native-kite-auth` |
| Live dashboard | dashboard Redis `kite-prod:v1:{<slot>}:dash…` |
| Logs | `logs/<strategy>-<mode>-<date>.jsonl` |

Each slot also runs as its own Nautilus trader (`kite-prod-<slot>`) with its own strategy ID.

### 3.2 Own settings file

All strategy inputs live in the file named by `strategy_config`. Two slots can run the same
strategy with different settings (for example, two Sniper configs on two instruments).

### 3.3 One process per slot per day

Each `native-<strategy>-paper` / `native-<strategy>-live` run handles one slot for one trading
day:
1. it checks the slot, the calendar, the contract and history;
2. it takes the slot's lock file;
3. it warms up on broker history;
4. it trades until the square-off;
5. it flattens and stops.

Paper runs use the native Kite **mock** execution client (live
market data, simulated fills, never Zerodha's order API).

### 3.4 Lock file and startup check

* A run takes the slot's lock file (an OS lock) for its whole life; a second process on the
  same slot and account is refused at once, naming the holder's PID. Preflight only checks
  that the lock is free.
* The OS frees the lock when the process ends for any reason, so a crash or kill never blocks
  the next start. There is no review step and no journal.
* Every start requires the Kite account to be **flat with no open orders** (production; read
  from Kite at connect). After an unclean stop, square off in Kite and start again.
* A run never resumes an earlier run.

### 3.5 Retention

Nothing to retain: since 2.21.0 no order journal or Nautilus cache is written to Redis.
Ledgers, leases and caches from runs before 2.21.0 are no longer read; they can be deleted
from Redis by hand (`kite-prod:v1:{<slot>}:commands:*`, `kite-prod:v1:{<slot>}:lease:*`,
`trader-kite-prod-<slot>:*`). Redis no longer needs `appendfsync always` for trading.

### 3.6 Order admission (kite-adapter 0.2.9)

The account is reconciled at startup (flat, no open orders) and then kept current: every
Kite order-stream update, plus a 15 s fallback, triggers a REST reconciliation (orders, trades,
positions, margins). An order is admitted **from that observation, without new REST reads**,
when all of these hold:

* the order stream is connected and has not reconnected since the observation;
* no order update arrived since the observation started (any order on the account, manual ones
  included);
* the observation is under 20 s old;
* no owned order is unresolved, and the strategy's position equals the observed position.

Otherwise the full REST preflight runs as before. Contract cap and exposure-shape checks run on
every order either way. A protective-stop modify skips its REST confirmation under the same
rule. A manual order placed just before an admitted order is still caught by the
reconciliation it triggers, which stops the run for review. The run log ends with
`{"event":"native_admissions","cached":N,"full_preflight":M}`.

Per order, the only write before sending is the shared order-rate budget in Redis (one
round trip to local Redis). The journal record and the lease attempt counter are gone (2.21.0).

### 3.7 Postback fills (kite-adapter 0.3.1)

A fill used to reach the strategy only through the full REST reconciliation: five sequential
reads (orders, trades, positions, margins, orders again), retried after 250 ms and 500 ms
while `/trades` lagged the order. Now a Kite order-stream postback with status `COMPLETE`
for an **owned** order first takes a fast path:

1. `GET /orders` (the day book, for that order) and `GET /orders/{id}/trades`, sent together:
   one round trip. The day book is used rather than `GET /orders/{id}` because Kite documents
   the history entries without `market_protection` and `exchange_update_timestamp`, which the
   checks need for MARKET orders.
2. They go through the same ownership, contract, quantity and chronology checks as the full
   reconciliation. The fills carry the **real Kite trade IDs**; the postback payload itself
   never creates a fill.
3. The order record is updated first, then `OrderAccepted` (if still pending) and
   `OrderFilled` are emitted.
4. The full reconciliation runs right after, as before. It must show the same trades, with
   unchanged quantity, price, time and order id. While `/trades` or positions still lag, the
   run continues for up to **30 s**; past that, or on any difference, it stops for review.
   A snapshot still missing a postback fill never backs cached admission.

The fast path is skipped, and the full reconciliation handles the update exactly as in
0.2.9, when: the order is not owned (manual orders), already closed, has a pending
protective-stop modification, is not `COMPLETE` in the day book yet, a read fails or takes
over 3 s, its trades do not add up yet, or any check fails. Partial-fill postbacks
(`UPDATE`/`OPEN`) are not fast-pathed. The mock (paper) broker does not use it.

Log events: `native_postback_fill` (with `ms` from the start of the reads),
`native_postback_fill_verified` (`after_ms`), `native_postback_fill_skipped` (`reason`), and
`postback_fills` in the final `native_admissions` line. A clean shutdown also requires every
postback fill to be verified.

---

## 4. Gates for real orders

A live run (`native-sniper-live` / `native-sats-live`) needs **all** of these:

| # | Gate | Where |
|---|---|---|
| 1 | Binary built with `--features live-orders` | `cargo build --release --features live-orders` |
| 2 | Slot `enabled: true` | portfolio manifest |
| 3 | Slot `live_orders_enabled: true` | portfolio manifest |
| 4 | Broker settings: `live_orders_enabled: true`, the exact Kite `expected_user_id`, `instrument_token` equal to the slot's, `product` MIS, `market_protection` −1, `max_lots` ≥ the strategy's lots (Sniper) | `config/kite-production.json` |
| 5 | Operator types `LIVE` | `deploy/run-*-live.sh` |

At connect time the production client also checks:
* the Kite profile (user ID, MCX enabled, MIS allowed);
* that the **account has no open position** at all.

`max_lots` (1–10, shipped 3) is the largest position the adapter will hold for the contract;
orders that would end beyond it are refused before reaching Kite. A flip order may be larger
than `max_lots` as long as the resulting position is within it.

The order budget is the program's own cap on Kite order calls (placements, modifications,
cancellations, failed attempts): **5 per second, 100 per minute, 1000 per day per Kite
account**, shared by every slot on that account.

---

## 5. Limitations

### 5.1 One live slot per Zerodha account at a time (most important)

Before every order, the adapter takes a snapshot of the **whole** Kite account and refuses the
order (the strategy then halts and flattens) if:
* there is a position in any other symbol or product ("Unmanaged account exposure");
* Kite's net position in the slot's contract differs from what this slot believes it holds;
* there is an open order the slot does not own.

Live startup also requires the account to be completely flat. As a result:
* **two slots cannot trade live on the same account at the same time**, even on different
  instruments: each would see the other's position as unmanaged exposure;
* Sniper and SATS on the same contract would also clash on the position check;
* **no manual trading in that account** while a slot is live. A manual position or order halts
  the bot.

This strict exclusivity is a deliberate design choice. Paper runs use the mock broker and do
not touch the real account, so a paper slot can run next to a live one.

### 5.2 One broker settings file per contract

`config/kite-production.json` holds one `instrument_token`, which must equal the slot's.
A second live instrument would need its own settings file (and, per 5.1, its own account).
`max_lots` is per settings file.

### 5.3 Fixed scope

* At most **4 slots** per manifest.
* **MCX and NFO** futures only.
* Product **MIS** only (intraday; everything is flat at the square-off).
* Only `sniper` and `sats` have runners. The NIFTY and GOLD slots are placeholders
  (`unassigned`, token 0) until a strategy is written and approved for them.

### 5.4 Manual rollover

There is no automatic roll. The runner refuses to start after `rollover.expected_expiry`, and
`next_contract` must stay a placeholder (unverified, unapproved, token 0).
Each month is a new slot (section 6).

### 5.5 Day-scoped runs

* A run must start inside the MCX session and at least 5 minutes before the square-off.
* It ends after the square-off.
* There is no overnight position, and no carry-over of the Nautilus cache between runs: each
  run starts flat and reconciles with Zerodha.

### 5.6 No crash recovery

If the process dies, nothing in the program protects an open position until you act: a SATS
SL-M already resting at Kite still protects it; a Sniper position has only MIS auto square-off.
Check positions and open orders in Kite, square off if needed, then start again; the startup
check refuses a non-flat account or one with open orders (section 3.4).

### 5.7 Leftovers

* `broker_config` in the manifest is not used (section 2.1).

---

## 6. Monthly rollover procedure (CRUDEOILM)

CRUDEOILM expires around the 19th; roll a few days before.

1. **Find the new contract** in the Kite instrument master: symbol (e.g. `CRUDEOILM26NOVFUT`),
   token, expiry.
2. **Add a new slot** in `config/portfolio-production.json` (keep the old one, set it
   `enabled: false`):
   * `id`: `crudeoilm-sniper-202611`;
   * `instrument`: `CRUDEOILM26NOVFUT.MCX`, plus the new `instrument_token`;
   * `rollover`: `strategy_id` `crudeoilm-sniper`, `contract_month` `2026-11`, the new
     `expected_expiry`, `lot_size` 1;
   * `enabled: true`, `live_orders_enabled: true`.
   (With 4 slots already listed, remove an unused placeholder first.)
3. **Update the strategy config**: `symbol` and `instrument_token` in
   `config/sniper-crudeoilm.json` (Sniper; SATS takes them from the slot).
4. **Update the broker settings**: `instrument_token` in `config/kite-production.json`.
5. **Update the launch script**: `SLOT=` in `deploy/run-sniper-*.sh`.
6. **Validate**: `native-portfolio-validate`, then `native-sniper-live-preflight`, then a
   paper run.

---

## 7. Adding a slot for a new strategy or instrument

1. Write the strategy (engine crate + config + live strategy + runner + backtest), following
   Sniper or SATS.
2. Add the slot with `enabled: false`, `live_orders_enabled: false`, a real token and its
   config file.
3. Backtest, then paper-trade (paper runs work while the slot is disabled).
4. Only then set `enabled: true` and `live_orders_enabled: true`. Remember 5.1: it cannot trade live at the same
   time as another slot on the same account.

---

Related: `doc/SNIPER_STRATEGY.md`, `doc/SATS_STRATEGY.md`.

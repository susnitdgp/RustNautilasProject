# Portfolio slots: features and limitations

The portfolio manifest (`config/portfolio-production.json`) lists the **slots** this project can
run. A slot is one strategy on one futures contract month, with its own settings file, its own
lock and its own logs. Code: `apps/kite-node/src/native_node/portfolio.rs`
(manifest and validation) and `crates/kite-adapter/src/execution/native_client/keys.rs`
(names).

Written for kite-node 2.25.0 / kite-adapter 0.8.0.

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
| Live dashboard | local Redis database 1, `kite-prod:v1:{<slot>}:dash…` |
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
`trader-kite-prod-<slot>:*`). Since 2.21.1 local Redis runs `appendfsync everysec`
(`redis-utility/redis-fsync-everysec.sh`): nothing in the order path waits for the disk.

### 3.6 Order path: place, reconcile, done (kite-adapter 0.7.0)

Since kite-adapter 0.7.0 / kite-node 2.24.0 the order path works like a standard Nautilus
venue adapter (e.g. Interactive Brokers): **placing an order makes no Kite read**.

**Placing.** Admission is local and in memory, then one place call:

* the order stream is connected (new entries only; exits always pass);
* no owned order is still unresolved (one at a time: Kite does not enforce reduce-only, so a
  resting protective stop and an exit, or two entries, could otherwise both fill);
* the strategy's position equals the position from the owned fills;
* contract cap (`max_lots`) and exposure shape: a reduce-only exit no larger than the
  position, an entry from flat, or one full flip;
* the shared order-rate budget in local Redis (~0.06 ms; entries only, exits bypass it).

**Reconciliation.** The order book and the trade book are read together (2 reads, one round
trip, ~20 ms) and every owned order is turned into Nautilus events (`OrderAccepted`,
`OrderFilled` with the real Kite trade IDs, `OrderCanceled`, `OrderRejected`). It runs on
every Kite order-stream update, every **2 s** while an owned order is unresolved, and every
15 s otherwise. If the two books disagree (a trade ahead of the order, or the reverse;
`ObservationLag`) the book is read up to 3 times, 150 ms apart; still disagreeing, nothing
is applied and the next pass reads again. A fill is never inferred. An open order nobody here
placed halts the run at once.

**Account audit.** Every 15 s positions are read (1 read):

* any position in another contract or product halts the run at once ("Unmanaged account
  exposure");
* Kite's position for this contract must equal the owned fills. Kite's positions trail its
  trade book by a moment, so a difference is tolerated for **20 s** (logged as
  `native_position_mismatch`, then `native_position_converged`); still different after that,
  the run halts.

**Start and stop.** Start-up still reads the full account (orders, trades, positions,
margins) and requires it flat with no open orders. At shutdown the book is reconciled until
nothing owned is unresolved (up to 5 passes, 500 ms apart), then positions must match the
owned fills and be flat (up to 5 reads, 1 s apart).

**What changed from 0.6.0.** Removed: the pre-order account snapshot and cached admission
(doorbell, 20 s freshness), the positions read on every reconciliation, the double order-book
read, the switched-off postback fast path (`Dispatcher::fast_fill`, `GET /orders/{id}/trades`)
and the protective-stop REST confirmation before a modify (Kite itself refuses to modify an
order that has filled or been cancelled; an uncertain answer stops the run). Trade-off: a
manual position in another contract is now found by the next audit (within 15 s) instead of
before the next order; an open manual order is still found by the next reconciliation.

Per order this is: 0 reads + 1 place call; then one reconciliation (2 reads) when Kite's
order update arrives. Until 0.6.0 it was two snapshots of 5 reads each; in 0.6.0 one snapshot
of 4 reads.

### 3.6b Latency log (kite-node 2.24.1)

Each run writes three kinds of JSON line to its log (stdout):

* `latency_bar`, one per live bar: `after_close_ms` (candle close to the bar reaching the
  strategy) and `tick_age_ms` (age of the tick that closed it; exchange time has 1 s
  resolution);
* `latency_order_sent`, one per order: `strategy_to_send_ms` (order created to place call
  out), `admission_ms`, `place_call_ms` (Kite round trip) and `outcome`;
* `latency_fill`, one per fill: `trigger` (`order_update`, `pending_timer`, `fallback_15s`,
  `reconnect`, `paper_poll_1s`, `direct`), `trigger_after_ack_ms` (for `order_update`: when
  Kite's update arrived), `read_ms`, `applied_after_trigger_ms`, `send_to_applied_ms` and
  `applied_after_kite_fill_ms` (approximate, 1 s resolution).

`python3 tools/latency-report.py logs/<run>.jsonl` prints count, median, p90 and max per
stage, plus candle close to fill applied. Paper runs use the simulated broker, so only the
bar and strategy stages are real there; the order and fill stages need a live run.

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
account**, shared by every slot on that account. Since kite-adapter 0.5.0, when the budget is
exhausted or Redis fails:
* a **new entry** (or flip) is denied (`OrderDenied`, the strategy decides what follows);
* an **exit** (reduce-only), a stop modification or a cancel still goes out, with a
  `native_budget_bypassed` line in the log (Kite's own limits are higher than this budget);
* after a Redis error the budget reconnects on the next order instead of refusing every order
  for the rest of the run. Before 0.5.0 either case stopped the whole run, open position
  included.

The Kite order stream (order updates) may drop and reconnect **3 times in any 10 minutes**,
with up to 3 connection attempts per drop (0.5 s, 1 s, 1.5 s apart). Beyond that the run
faults, flattens and stops. Before 0.5.0 it was 2 reconnects per run and a single attempt.

---

## 5. Limitations

### 5.1 One live slot per Zerodha account at a time (most important)

The adapter watches the **whole** Kite account (section 3.6) and halts the run (the strategy
then flattens) if:
* there is a position in any other symbol or product ("Unmanaged account exposure", account
  audit every 15 s);
* Kite's net position in the slot's contract differs from the owned fills for more than 20 s;
* there is an open order the slot does not own (every reconciliation).

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

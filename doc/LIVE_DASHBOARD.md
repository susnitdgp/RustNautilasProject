# Live dashboard data (Redis)

A running Sniper or SATS slot publishes its live state to a **separate dashboard Redis**
(never the trading Redis), for a web dashboard to read. Written for kite-node 2.18.0 /
kite-adapter 0.2.8.

## 1. How it works

```
strategy  ──try_push──▶ rtrb queue ─┐
stop watcher ─try_push─▶ rtrb queue ─┼─▶ "dashboard" OS thread ──pipeline──▶ dashboard Redis
                                     │    owns the Board, applies updates
```

* The strategy never waits on the dashboard. Each update is a lock-free push into an SPSC
  queue (4096 slots); if a queue is ever full the update is dropped and counted
  (`dropped` in the snapshot; should stay 0).
* One thread (`dashboard`) owns the board, applies the updates and writes to Redis at most
  every 250 ms and at least once a second (heartbeat), in one pipelined round trip.
* Redis slow or down only delays the dashboard: the thread retries every 5 s, keeps the
  latest state and up to 500 unsent events, and reports on stderr when the connection
  drops or comes back. Order flow is unaffected.
* At stop the thread does a final write and the runner prints the board once on stderr.
* The terminal (ratatui) screen was removed.

## 2. Configuration

`config/dashboard.json` (gitignored, mode 600: the URL holds a password):

```json
{ "redis_dashboard_url": "redis://default:PASSWORD@HOST:PORT" }
```

Template: `config/dashboard.example.json`. Without the file the bot runs normally and
publishes nothing. Use `rediss://` once TLS is enabled on the database (the current Redis
Cloud database accepts plain `redis://` only, so traffic is unencrypted). No Kite
credentials are ever written to this Redis; the URL is never logged.

## 3. Keys

Base: `<redis_prefix>:v1:{<slot>}:dash`, e.g. `kite-prod:v1:{crudeoilm-sniper-202610}:dash`
(the portfolio key scheme; the startup JSON log line `*_node_started` lists all three keys).
Keys are per slot, so the dashboard always shows the slot's latest run.

| Key | Type | Content |
|---|---|---|
| `<base>:state` | hash | `snapshot` (JSON, below), `updated_at_ms`, `seq`, `status`. Expires 7 days after the last write |
| `<base>:events` | stream | one entry per event: `ts_ms` (epoch ms), `text`. Capped at ~1000 (`XADD MAXLEN ~`). Expires 7 days after the last event |
| `<base>:live` | pub/sub channel | message = `seq` after every write: subscribe to refresh immediately |

Staleness: `updated_at_ms` advances at least once a second while the bot runs; older than a
few seconds = the bot is stopped or cannot reach this Redis.

## 4. Snapshot JSON (`HGET <base>:state snapshot`)

```json
{ "seq": 412, "updated_at_ms": 1791627127000, "board": { … } }
```

`board` fields:

| Field | Meaning |
|---|---|
| `title`, `mode`, `status`, `slot`, `instrument` | Header: strategy title, `LIVE (real Zerodha orders)` / `PAPER (Kite mock execution)`, `STARTING` / `RUNNING` / `STOPPING` / `SQUARED OFF` / `HALTED`, slot id, instrument |
| `halted` | Halt reason or `null` |
| `last_price`, `last_bar` | Last quote; last closed bar as `[label, close]` |
| `history_bars`, `live_bars`, `bar_ns` | Bar counts; bar length in ns (next-bar countdown = next multiple of `bar_ns`) |
| `feed_fault` | Market-data fault or `null` |
| `warmed`, `trend` | Model warmed up; trend 1 / -1 / 0 |
| `model_title`, `model_rows` | Model panel title and `[label, value]` rows (Sniper: EMAs, score, ADX/RSI). Empty rows = SATS panel |
| `supertrend`, `tqi`, `regime`, `next_r` | SATS panel values |
| `exit_rule` | Exit plan text |
| `position`, `entry_avg` | Signed lots from broker fills; average entry |
| `sl`, `tps`, `exchange_stop` | Model stop, `[TP1, TP2, TP3]`, resting SL-M trigger (SATS) |
| `realized_points`, `round_trips`, `fills`, `point_value`, `lots` | Realised P&L in points (× `point_value` = ₹), counts, configured lots. Unrealised = `position × (last_price − entry_avg)` |
| `square_off`, `redis_namespace` | Square-off time (IST, "HH:MM"); the run ID (`YYYYMMDD-xxxxxxxx`) |
| `events` | Last 8 events as display strings (`"HH:MM:SS  text"`, newest first); the full history is in the stream |
| `dropped` | Dashboard updates dropped because a queue was full |

## 5. Reading it

```bash
redis-cli -u "$URL" HGET 'kite-prod:v1:{crudeoilm-sniper-202610}:dash:state' snapshot
redis-cli -u "$URL" XREVRANGE 'kite-prod:v1:{crudeoilm-sniper-202610}:dash:events' + - COUNT 20
redis-cli -u "$URL" SUBSCRIBE 'kite-prod:v1:{crudeoilm-sniper-202610}:dash:live'
```

A web dashboard: read the snapshot and the latest stream entries on page load, then
subscribe to `:live` (via a small server-side relay or WebSocket) and re-read the snapshot on
each message. Keep the Redis password on the server side, never in browser code.

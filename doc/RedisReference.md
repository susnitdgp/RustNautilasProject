# Redis keys and state used by RustNautilasProject

This document covers the Redis keys used by the selected CRUDEOIL Supertrend + MACD + VWAP LiveNode, native Kite execution, simulations, credentials, alerts, and Nautilus cache. Redis database 0 is required. The application uses `KITE_REDIS_URL` when set and otherwise connects to `redis://127.0.0.1:6379/0`.

## Rules

- Never run `FLUSHDB`, `FLUSHALL`, or delete keys by a broad wildcard.
- Never read credentials into terminal logs. Check them with `EXISTS`, not `GET`.
- Never delete an owner or `ReviewRequired` record while the process is running.
- Before releasing a failed run, compare the native journal with Kite orders, trades, and positions. Confirm zero open broker orders and the actual broker position.
- Preserve the command journal, health record, run report, and manual-review audit after a failure.
- Use `SCAN`, not `KEYS`, on the production Redis instance.

## Credentials and optional notifications

| Key | Type | Purpose | Handling |
|---|---|---|---|
| `susanta:kite_api_key` | string | Production Kite API key | Secret; application-managed input |
| `susanta:kite_access_token` | string | Production Kite session token | Secret; normally expires at the next daily session boundary |
| `sandbox:kite_api_key` | string | Kite sandbox API key | Secret; never falls back to production credentials |
| `sandbox:kite_access_token` | string | Kite sandbox access token | Secret; never falls back to production credentials |
| `susanta:slack_webhook_url` | string | Optional Slack incoming-webhook URL | Secret; used only when `KITE_SLACK_ALERTS=1` |

Presence check:

```bash
redis-cli EXISTS susanta:kite_api_key susanta:kite_access_token
redis-cli EXISTS sandbox:kite_api_key sandbox:kite_access_token
redis-cli EXISTS susanta:slack_webhook_url
```

## Strategy ownership

Only one selected strategy process may own a given mode.

| Key | Type | Value |
|---|---|---|
| `kite:production:supertrend:CRUDEOIL26OCTFUT:owner` | string | Production run UUID |
| `kite:paper:supertrend:CRUDEOIL26OCTFUT:owner` | string | Live-data paper run UUID |
| `kite:paper:supertrend:sim:<RUN_UUID>:owner` | string | Isolated synthetic run UUID |
| `<OWNER_KEY>:heartbeat` | hash | `owner`, `last_seen_ns` |

The owner is acquired with `SET ... NX`. There is no TTL and no automatic takeover. A clean shutdown deletes only an owner that still matches its run UUID. A crash or review-required shutdown retains it.

Read-only checks:

```bash
redis-cli --raw GET kite:production:supertrend:CRUDEOIL26OCTFUT:owner
redis-cli HGETALL kite:production:supertrend:CRUDEOIL26OCTFUT:owner:heartbeat
```

## Strategy run health

| Key pattern | Type | Fields |
|---|---|---|
| `kite:production:supertrend:<RUN_UUID>:health` | hash | `state`, `position`, `owner_key`, `live_orders_enabled` |
| `kite:paper:supertrend:<RUN_UUID>:health` | hash | Same fields for paper execution |

`state` is `Clean` or `ReviewRequired`. `position` is the last locally known signed quantity. It is diagnostic and does not replace a broker position query. `live_orders_enabled` records how the completed run was configured; changing it does not enable orders.

## Native account coordination

| Key pattern | Type | Purpose |
|---|---|---|
| `susanta:nautilus:native-kite:account:{<ACCOUNT_ID>}` | hash | Account-wide single owner, command admission, heartbeat, and recovery state |

Fields:

| Field | Meaning |
|---|---|
| `scope` | Schema marker (`NATIVE_DISABLED_V1` in the current implementation; the name is historical and is not an enable switch) |
| `owner` | Run UUID; empty only after a clean release or reviewed manual recovery |
| `state` | `Starting`, `Dispatching`, `Running`, `Stopping`, `Clean`, or `ReviewRequired` |
| `unresolved` | Count of native orders that are not closed |
| `position` | Last reconciled signed broker quantity |
| `heartbeat_ms` | Redis server time of the last coordination update |
| `last_namespace` | Most recent run UUID |
| `command_attempts` | Commands admitted before broker transport; includes attempts that may fail or time out |
| `manual_review_namespace` | Run UUID released after explicit broker/native review, when applicable |

The coordination key has no TTL. `ReviewRequired`, a non-empty owner, an unresolved order, or uncertain release blocks restart. Use the application command for status:

```bash
./target/release/kite-node native-kite-status <ACCOUNT_ID>
```

## Durable native command journal

| Key pattern | Type | Purpose |
|---|---|---|
| `susanta:nautilus:native-kite:commands:{<RUN_UUID>}` | hash | Commands and broker observations persisted before/after execution |

Hash entries include:

| Entry | Meaning |
|---|---|
| `scope` | Journal schema marker (`NATIVE_KITE_DISABLED_V1`; historical name, not an enable switch) |
| `order:<CLIENT_ORDER_ID>` | JSON `Record` containing `events`, `tag`, `product`, `token`, `broker_id`, `outcome`, and `management` |

Common outcomes include `Acknowledged`, `Observed`, `Rejected`, `RateLimited`, `SessionExpired`, and `Unknown`. An acknowledgement proves receipt only. Fills are recorded from matching broker trades.

Review commands:

```bash
./target/release/kite-node native-kite-review <RUN_UUID>
./target/release/kite-node native-recover <RUN_UUID>
```

## Manual recovery audit

| Key pattern | Type | Purpose |
|---|---|---|
| `susanta:nautilus:native-kite:manual-review:{<RUN_UUID>}` (or a unique `:<REVIEW_UUID>` suffix) | hash | Evidence retained when an exact stale owner is released after broker review |

Current fields include `namespace`, `reviewed_at_ms`, `broker_open_orders`, `broker_position`, `manual_closure`, `previous_state`, `previous_unresolved`, and `previous_position`.

This key is evidence. Do not delete it during routine cleanup.

## Account rate limits

The account coordinator opens a persistent limiter for scope `native-account-<ACCOUNT_ID>`. The current application policy is five commands per second, 100 per minute, and 1,000 per day. It also stores broker-directed cooldowns. The exact Redis key is generated by `kite-execution::rate_limit::Limiter::key`; treat every limiter/budget key as application-managed and preserve it across restarts.

Simulation budgets use keys such as:

- `susanta:nautilus:sim:order-budget:{<RUN_UUID>}`
- `susanta:nautilus:sim:journal:{<RUN_UUID>}`

These are test-only when the UUID belongs to a verified completed simulation. Production account limiter keys must not be removed to bypass admission or cooldown.

## Nautilus native cache

Nautilus persists each node instance under:

`trader-SUSANTA-001:<INSTANCE_UUID>:<CATEGORY>:<IDENTIFIER>`

Observed categories include:

| Category/key suffix | Data |
|---|---|
| `accounts:<ACCOUNT_ID>` | Native account state and account events |
| `orders:<CLIENT_ORDER_ID>` | Native order and event history |
| `positions:<POSITION_ID>` | Native positions |
| `instruments:<INSTRUMENT_ID>` | Instrument definition |
| `currencies:<CODE>` | Currency definition |
| `strategies:<STRATEGY_ID>:state` | Strategy state |
| `actors:<ACTOR_ID>:state` | Actor state |
| `index:orders`, `index:orders_open`, `index:orders_closed`, `index:orders_inflight` | Order indexes |
| `index:positions`, `index:positions_open`, `index:positions_closed` | Position indexes |
| `index:order_ids`, `index:order_client`, `index:order_position` | Identity/correlation indexes |
| `general:cache://position-snapshots/...` | Native position snapshots |
| `general:position_oms:<POSITION_ID>` | Position OMS state |

These keys are required for `native-recover`. Do not edit individual fields or delete part of an instance. An incomplete instance can make reconstruction incorrect.

## Backtest-specific state

Backtests may create strategy-specific state such as `kite:backtest:supertrend-stop:<...>` and complete Nautilus instance caches. Saved reports live on disk under `backtest_results/`; deleting Redis does not delete reports. Cleanup may remove a whole old backtest namespace only when its report says completed, its native indexes show no open/inflight orders or open positions, and recent/reference namespaces are preserved.

## What enables real orders

No Redis flag alone enables production trading. Real orders require all of these:

1. A binary built with `kite-adapter/live-orders`.
2. `live_orders_enabled: true` in `config/kite-production.json`.
3. A valid expected Kite user ID, `product: MIS`, the exact token selected in `config/production-supertrend.json` (currently `145894407`), and `market_protection: -1`.
4. A valid production credential pair in Redis.
5. Clean strategy and account ownership state.
6. Successful account, permissions, funds-ledger, order, trade, and position checks.

`config/production-supertrend.json` is the reviewed strategy selection and keeps `live_orders_enabled: false`; that field is not the broker enable switch.

## Safe inspection

```bash
# Count keys without blocking Redis
redis-cli --scan | wc -l

# Show key names for one exact run
redis-cli --scan --pattern 'trader-SUSANTA-001:<RUN_UUID>:*'
redis-cli --scan --pattern 'susanta:nautilus:native-kite:*{<RUN_UUID>}*'

# Inspect types before reading
redis-cli TYPE '<EXACT_KEY>'
redis-cli HGETALL '<EXACT_NON_SECRET_HASH_KEY>'

# Verify persistence policy
redis-cli CONFIG GET appendonly appendfsync maxmemory-policy
redis-cli INFO persistence
```

Do not use `GET` on credential or webhook keys in copied terminal output.


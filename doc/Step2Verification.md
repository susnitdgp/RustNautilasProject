# Step 2: authenticated MCX market-data diagnostics

Current checkout: /home/ubuntu/RustNautilasProject on ip-172-31-36-59.

## Components

| Component | Rust module |
| --- | --- |
| Redis credential loading | crates/kite-adapter/src/credentials/redis.rs |
| Session validation and exchange permission | crates/kite-adapter/src/auth/session.rs |
| Binary packet models | crates/kite-adapter/src/websocket/models.rs |
| MCX binary packet decoding | crates/kite-adapter/src/websocket/parser.rs |
| Subscription intent | crates/kite-adapter/src/websocket/subscription.rs |
| Socket connection and timeouts | crates/kite-adapter/src/websocket/transport.rs |
| Reconnects, gap events and freshness summary | crates/kite-adapter/src/websocket/supervisor.rs |
| Exact MCX price scaling and snapshots | crates/kite-adapter/src/mapping/market_data.rs |
| CLI argument parsing | apps/kite-node/src/cli.rs |
| Instrument preflight commands | apps/kite-node/src/preflight_command.rs |
| Session and stream commands | apps/kite-node/src/market_data_command.rs |

## Manual verification

Run from the development server:

```bash
cd /home/ubuntu/RustNautilasProject
cargo test --locked --workspace
cargo run --locked -p kite-node -- session-check config/crudeoil-september.toml
cargo run --locked -p kite-node -- stream config/crudeoil-september.toml --seconds 15
```

Session check should report kite_session_validated=true and exchange_enabled=MCX.
The stream resolves the current instrument master, validates the stored session,
subscribes to CRUDEOIL26SEPFUT in full mode, and then automatically closes.
Compare LTP, bid and ask with the same standard CRUDEOIL SEP FUT instrument in
Kite at the same time. Prices can move between the two screens.

The command prints up to five sample snapshots, connection/gap events and one
summary. It continues counting and checking all received snapshots after the
sample display limit. This is not a market-data recorder.

Expect:
- full_ticks > 0;
- final_source_fresh = true during an active feed;
- live_orders_enabled = false;
- normally one connection generation and zero gaps on a healthy connection.

A nonzero exit status means verification was incomplete or failed. No fresh data
outside market hours is not proof of a parser defect. Duration must be 1..300
seconds. The duration covers the stream run; Redis, instrument download and
session validation occur before it, and close can take up to one extra second.
Ctrl+C cancels the observation and drops its socket.

## Protocol and failure behavior

The MCX parser supports the 8-, 44- and 184-byte Kite packet layouts. It checks
frame lengths, packet count, truncation and unexpected tokens. It decodes all
five bid and ask levels in full packets; CLI snapshots show only the best level.
Prices use exact paise-to-rupee decimal scaling. Cumulative volume is preserved
as cumulative volume, not interpreted as an individual trade.

A zero source timestamp remains absent. Freshness requires a source timestamp
between two seconds ahead and ten seconds behind the local receive/check time.
Keep the server clock synchronized. Heartbeats alone never establish fresh data.

At most one socket is open in this diagnostic. Network disconnect/idle timeout
invalidates the current generation and emits a gap. Up to two reconnects replay
subscription and full-mode intent with a short increasing delay plus jitter.
A new generation must receive a full snapshot before it can be fresh. Missing
events are not reconstructed. The account/API-wide three-connection limit also
includes other applications; this CLI does not coordinate their connections.

Initial/handshake errors, server error text, malformed protocol or wrong tokens
stop the run. The diagnostic does not retry authentication, log response bodies,
print credential URLs, or automatically renew tokens. A subsequent run reloads
Redis. The public CLI always uses the official Kite endpoints.

Order postbacks can arrive on Kite's shared socket; this data-only stage counts
and discards them without printing their payloads. It does not maintain order
state, issue orders or provide execution reconciliation.

## Automated verification and live result

15 September 2026 on ip-172-31-36-59:
- 47 tests passed, zero failures.
- Clippy passed with warnings denied.
- Formatting and whitespace checks passed.
- Real session validation passed and confirmed MCX enabled.
- A 15-second live observation beginning around 07:13:50 UTC received 15 full
  snapshots and four heartbeats, with zero reconnect gaps.
- All 15 full snapshots were fresh at receipt, and the final source was fresh.
- First sample: LTP 9917.00, bid 9916.00, ask 9917.00.
- Instrument: CRUDEOIL26SEPFUT.MCX, token 144870151.
- No live orders were enabled or sent.

Tests include authentication headers/403 handling, redacted errors, full depth
offsets, truncated packets, missing timestamps, exact scaling, heartbeat-only
runs, bounded shutdown, forced disconnection, subscription replay and exhausted
reconnect budgets. Reconnect tests use local mock sockets, not deliberate
disruption of the live broker connection.

## Next checkpoint

After manual Step 2 verification, implement the Nautilus DataClient/factory,
full futures instrument mapping and recording/replay. Contract multiplier and
Kite quantity semantics must be validated before constructing the full economic
instrument and before any order-sizing work. The Nautilus engine is not started
by this diagnostic stage.

## Protocol references

- [Kite WebSocket protocol](https://kite.trade/docs/connect/v3/websocket/)
- [Kite user/profile authentication](https://kite.trade/docs/connect/v3/user/)

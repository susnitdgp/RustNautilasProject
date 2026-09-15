# Redis credential module

Location: crates/kite-adapter/src/credentials/redis.rs.
Secret container: crates/kite-adapter/src/credentials/mod.rs.

Preflight now requires both values from Redis:
- susanta:kite_api_key
- susanta:kite_access_token

Default endpoint: redis://127.0.0.1:6379/0.
Override through KITE_REDIS_URL in the local process environment when needed.
The Redis client supports redis:// and rediss:// (TLS with system roots).
Do not put passwords or credential values in version-controlled configuration,
command-line arguments, shell history, chat, screenshots or logs.

Both keys are read with one MGET. This is a consistent Redis read, but writers
must also update the pair atomically to avoid publishing mismatched credentials.
The program does not SET, DELETE, refresh or otherwise modify these keys.
Missing/non-string/empty/whitespace/control-containing values fail preflight.
Connection and socket read/write timeouts are three seconds; DNS resolution is
subject to the host resolver and is not a strict total-operation deadline.

Credentials have redacted Debug output, no serialization implementation, private
fields and explicit accessors for future authenticated transports. Owned secret
strings are zeroized on drop; this is not a guarantee that no transport/library
buffers ever held copies. Errors exclude raw Redis replies and credential URLs.

JSON reports credentials_loaded=true and kite_session_validated=false.
Loading a stored token does not establish that the token is still valid.
The credentials are loaded before the public instrument download; both CLI input
modes now require Redis. Library-only instrument tests remain independent of Redis.

Verification from the current checkout:

```bash
cargo test --locked --workspace
cargo run --locked -p kite-node -- preflight config/crudeoil-september.toml --download
```

No Kite authentication request, WebSocket session or order submission is performed.
This synchronous reader is for the standalone preflight/startup boundary only.
The live runtime must obtain credentials outside its core event loop.

## Verification result

15 September 2026, ip-172-31-36-59: 26 tests passed; Clippy passed
with warnings denied. Full preflight loaded both keys from local Redis and
resolved CRUDEOIL26SEPFUT.MCX at 07:00:00 UTC. Output contained only
credentials_loaded=true and kite_session_validated=false, with no secret values.

The later session-check and stream commands do validate the stored credentials
with Kite; see doc/Step2Verification.md. Plain preflight remains an instrument
and Redis check and continues to report kite_session_validated=false.

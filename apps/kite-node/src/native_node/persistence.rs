use anyhow::{Result, anyhow, ensure};
use nautilus_common::{cache::CacheConfig, enums::SerializationEncoding};
use nautilus_infrastructure::redis::cache::RedisCacheConfig;

/// Batching window for Nautilus cache writes. The writes run on Nautilus' own background
/// task, never the trading thread; batching sends them as one pipeline per window, so with
/// `appendfsync always` they cause far fewer AOF fsyncs next to kite-journal's pre-order
/// WAITAOF. A crash can lose at most this window of cache writes; kite-journal (WAITAOF)
/// and broker reconciliation remain the durable record.
const CACHE_BUFFER_INTERVAL_MS: usize = 100;

/// Uses the native cache backing, with a unique run prefix and no database flush.
/// MessagePack payloads (smaller than JSON). Runs written before kite-node 2.15.2 are
/// JSON and cannot be read back by `native-recover` with this encoding.
pub fn cache_config() -> CacheConfig {
    CacheConfig {
        encoding: SerializationEncoding::MsgPack,
        buffer_interval_ms: Some(CACHE_BUFFER_INTERVAL_MS),
        use_instance_id: true,
        flush_on_start: false,
        save_market_data: false,
        ..Default::default()
    }
}
pub fn redis_config() -> Result<RedisCacheConfig> {
    let url = kite_journal::connection::url_from_env()?;
    let client = redis::Client::open(url.as_str())
        .map_err(|_| anyhow!("Invalid Redis connection configuration"))?;
    let info = client.get_connection_info();
    ensure!(
        info.redis.db == 0,
        "Native Redis cache currently requires database 0"
    );
    let (host, port, ssl) = match &info.addr {
        redis::ConnectionAddr::Tcp(h, p) => (h.clone(), *p, false),
        redis::ConnectionAddr::TcpTls { host, port, .. } => (host.clone(), *port, true),
        _ => return Err(anyhow!("Native Redis cache requires TCP")),
    };
    let mut con = client
        .get_connection()
        .map_err(|_| anyhow!("Redis unavailable"))?;
    let aof: Vec<String> = redis::cmd("CONFIG")
        .arg("GET")
        .arg("appendonly")
        .query(&mut con)
        .map_err(|_| anyhow!("Cannot verify Redis AOF"))?;
    ensure!(
        aof.get(1).is_some_and(|v| v == "yes"),
        "Redis AOF must be enabled"
    );
    Ok(RedisCacheConfig {
        host: Some(host),
        port: Some(port),
        ssl,
        username: info.redis.username.clone(),
        password: info.redis.password.clone(),
        number_of_retries: 2,
        connection_timeout: 10,
        response_timeout: 10,
        ..Default::default()
    })
}

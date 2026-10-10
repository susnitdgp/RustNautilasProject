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
/// Retention for an earlier run's Nautilus cache (written under `trader-<id>:<instance>:`).
pub const PREVIOUS_RUN_CACHE_TTL_SECS: u64 = 7 * 24 * 3600;

/// Before a new node starts (so none of these keys belong to it), give this trader's earlier
/// run caches a retention TTL. Each run writes a fresh instance namespace that is never read
/// again, so without this they accumulate. Keys that already have a TTL are left unchanged.
/// Call only after the account lease startup check passed (no run of this slot is live).
pub fn expire_previous_runs(trader_id: &str) -> Result<usize> {
    expire_previous_runs_at(&kite_journal::connection::url_from_env()?, trader_id)
}
fn expire_previous_runs_at(url: &str, trader_id: &str) -> Result<usize> {
    ensure!(
        !trader_id.is_empty()
            && trader_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "Invalid trader id for cache retention"
    );
    let mut con = redis::Client::open(url)
        .and_then(|c| c.get_connection())
        .map_err(|_| anyhow!("Redis unavailable"))?;
    let pattern = format!("trader-{trader_id}:*");
    let mut cursor = 0_u64;
    let mut changed = 0_usize;
    loop {
        let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(&pattern)
            .arg("COUNT")
            .arg(1000)
            .query(&mut con)
            .map_err(|_| anyhow!("Cache retention scan failed"))?;
        if !keys.is_empty() {
            let mut pipe = redis::pipe();
            for key in &keys {
                pipe.cmd("EXPIRE")
                    .arg(key)
                    .arg(PREVIOUS_RUN_CACHE_TTL_SECS)
                    .arg("NX");
            }
            let set: Vec<i32> = pipe
                .query(&mut con)
                .map_err(|_| anyhow!("Cache retention update failed"))?;
            changed += set.iter().filter(|v| **v == 1).count();
        }
        if next == 0 {
            return Ok(changed);
        }
        cursor = next;
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

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../../crates/kite-journal/test-support/redis.rs"]
mod test_redis;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earlier_run_caches_get_a_ttl_once_and_other_traders_are_untouched() {
        let r = super::test_redis::TestRedis::new();
        let mut c = r.connection();
        for key in [
            "trader-kite-prod-slot-a:uuid-1:orders:O-1",
            "trader-kite-prod-slot-a:uuid-2:accounts:KITE-X",
            "trader-kite-prod-slot-ab:uuid-3:orders:O-1",
            "kite-prod:v1:{slot-a}:lease:X",
        ] {
            let _: () = redis::cmd("SET").arg(key).arg("v").query(&mut c).unwrap();
        }
        assert_eq!(expire_previous_runs_at(&r.url, "kite-prod-slot-a").unwrap(), 2);
        assert_eq!(expire_previous_runs_at(&r.url, "kite-prod-slot-a").unwrap(), 0);
        let ttl = |c: &mut redis::Connection, k: &str| -> i64 {
            redis::cmd("TTL").arg(k).query(c).unwrap()
        };
        assert!(ttl(&mut c, "trader-kite-prod-slot-a:uuid-1:orders:O-1") > 6 * 86400);
        assert_eq!(ttl(&mut c, "trader-kite-prod-slot-ab:uuid-3:orders:O-1"), -1);
        assert_eq!(ttl(&mut c, "kite-prod:v1:{slot-a}:lease:X"), -1);
        assert!(expire_previous_runs_at(&r.url, "bad*id").is_err());
    }
}

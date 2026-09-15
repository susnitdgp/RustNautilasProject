//! Redis connection settings; URLs and authentication errors are never echoed.
use anyhow::{Result, anyhow, ensure};
use std::{env, time::Duration};
use zeroize::Zeroizing;
pub fn url_from_env() -> Result<Zeroizing<String>> {
    Ok(Zeroizing::new(match env::var("KITE_REDIS_URL") {
        Ok(url) => url,
        Err(env::VarError::NotPresent) => "redis://127.0.0.1:6379/0".into(),
        Err(_) => return Err(anyhow!("Invalid KITE_REDIS_URL")),
    }))
}
pub(crate) fn connect(url: &str) -> Result<redis::Connection> {
    let client =
        redis::Client::open(url).map_err(|_| anyhow!("Invalid Redis journal configuration"))?;
    let mut connection = client
        .get_connection_with_timeout(Duration::from_secs(3))
        .map_err(|_| anyhow!("Redis journal connection failed"))?;
    connection
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| anyhow!("Redis journal timeout setup failed"))?;
    connection
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| anyhow!("Redis journal timeout setup failed"))?;
    let policy: Vec<String> = redis::cmd("CONFIG")
        .arg("GET")
        .arg("maxmemory-policy")
        .query(&mut connection)
        .map_err(|_| anyhow!("Cannot verify Redis eviction policy"))?;
    ensure!(
        policy.get(1).map(String::as_str) == Some("noeviction"),
        "Redis journal requires noeviction"
    );
    sync(&mut connection)?;
    Ok(connection)
}
pub(crate) fn sync(connection: &mut redis::Connection) -> Result<()> {
    let (local, _replicas): (u32, u32) = redis::cmd("WAITAOF")
        .arg(1)
        .arg(0)
        .arg(2000)
        .query(connection)
        .map_err(|_| {
            anyhow!("Redis AOF confirmation failed; require Redis 7.2+ and appendonly yes")
        })?;
    ensure!(local == 1, "Redis AOF sync timed out; reopen and reconcile");
    Ok(())
}

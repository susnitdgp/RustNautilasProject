//! Check durable ownership off the trading thread; lost Redis/ownership stops admission.
use super::live_control::Control;
use anyhow::{Result, ensure};
use std::{sync::atomic::Ordering, time::Duration};
pub fn start(key: String, owner: String, control: Control) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while !control.stopping.load(Ordering::Acquire) {
            let key = key.clone();
            let owner = owner.clone();
            let result = tokio::task::spawn_blocking(move || check(&key, &owner)).await;
            if !matches!(result, Ok(Ok(()))) {
                control.fail("Redis ownership check failed; review required");
                break;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })
}
fn check(key: &str, owner: &str) -> Result<()> {
    let client = redis::Client::open(kite_journal::connection::url_from_env()?.as_str())?;
    let mut con = client.get_connection_with_timeout(Duration::from_secs(2))?;
    con.set_read_timeout(Some(Duration::from_secs(2)))?;
    con.set_write_timeout(Some(Duration::from_secs(2)))?;
    let script = "if redis.call('GET',KEYS[1])~=ARGV[1] then return 0 end redis.call('HSET',KEYS[2],'last_seen_ns',ARGV[2],'owner',ARGV[1]);return 1";
    let n: i64 = redis::cmd("EVAL")
        .arg(script)
        .arg(2)
        .arg(key)
        .arg(format!("{key}:heartbeat"))
        .arg(owner)
        .arg(super::data::now())
        .query(&mut con)?;
    ensure!(n == 1, "Strategy ownership lost");
    Ok(())
}

//! Single paper-strategy owner; an unclean run retains its Redis ownership.
use anyhow::{Result, ensure};
pub struct Lease {
    con: redis::Connection,
    key: String,
    owner: String,
    real: bool,
}
impl Lease {
    pub fn acquire(owner: &str, sim: bool, real: bool, symbol: &str) -> Result<Self> {
        kite_adapter::instruments::contract::validate_symbol(symbol)?;
        let mut con = redis::Client::open(kite_journal::connection::url_from_env()?.as_str())?
            .get_connection()?;
        let key = if real {
            format!("kite:production:supertrend:{symbol}:owner")
        } else if sim {
            format!("kite:paper:supertrend:sim:{owner}:owner")
        } else {
            format!("kite:paper:supertrend:{symbol}:owner")
        };
        let set: Option<String> = redis::cmd("SET")
            .arg(&key)
            .arg(owner)
            .arg("NX")
            .query(&mut con)?;
        ensure!(
            set.is_some(),
            "Paper Supertrend owner exists; review prior run before restarting"
        );
        Ok(Self {
            con,
            key,
            owner: owner.into(),
            real,
        })
    }
    pub fn monitor(&self, control: super::live_control::Control) -> tokio::task::JoinHandle<()> {
        super::owner_monitor::start(self.key.clone(), self.owner.clone(), control)
    }
    pub fn finish(&mut self, clean: bool, position: f64) -> Result<()> {
        let kind = if self.real { "production" } else { "paper" };
        let record = format!("kite:{kind}:supertrend:{}:health", self.owner);
        redis::cmd("HSET")
            .arg(record)
            .arg("state")
            .arg(if clean { "Clean" } else { "ReviewRequired" })
            .arg("position")
            .arg(position)
            .arg("owner_key")
            .arg(&self.key)
            .arg("live_orders_enabled")
            .arg(if self.real { "true" } else { "false" })
            .query::<()>(&mut self.con)?;
        if clean {
            let n:i64=redis::cmd("EVAL").arg("if redis.call('GET',KEYS[1]) == ARGV[1] then return redis.call('DEL',KEYS[1]) else return 0 end").arg(1).arg(&self.key).arg(&self.owner).query(&mut self.con)?;
            ensure!(n == 1, "Paper ownership lost");
        }
        Ok(())
    }
}

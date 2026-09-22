//! Persistent single account owner. No TTL takeover and no automatic unlock after a crash.
use anyhow::{Result, anyhow, ensure};
use kite_execution::rate_limit::{Decision, Limiter, policy::Policy};
use kite_journal::connection;
use std::collections::BTreeMap;

pub(crate) struct Account {
    connection: redis::Connection,
    key: String,
    owner: String,
    limiter: Limiter,
    poisoned: bool,
}
pub fn key(account: &str) -> Result<String> {
    ensure!(
        !account.is_empty()
            && account.len() <= 32
            && account.bytes().all(|b| b.is_ascii_alphanumeric()),
        "Invalid native account scope"
    );
    Ok(format!(
        "susanta:nautilus:native-kite:account:{{{account}}}"
    ))
}
/// Read-only admission check before acquiring a strategy lease. The atomic
/// Account::acquire remains authoritative if another process starts afterwards.
pub fn check_startup(account: &str) -> Result<()> {
    check_startup_at(&connection::url_from_env()?, account)
}
pub fn check_startup_at(url: &str, account: &str) -> Result<()> {
    let key = key(account)?;
    let mut con = connection::connect(url)?;
    let (values, ttl): (BTreeMap<String, String>, i64) = redis::pipe()
        .atomic()
        .cmd("HGETALL")
        .arg(&key)
        .cmd("PTTL")
        .arg(&key)
        .query(&mut con)
        .map_err(|_| anyhow!("Cannot verify native account startup state"))?;
    validate_startup(&values, ttl)
}
fn validate_startup(values: &BTreeMap<String, String>, ttl: i64) -> Result<()> {
    if values.is_empty() && ttl == -2 {
        return Ok(());
    }
    ensure!(
        values.get("scope").map(String::as_str) == Some("NATIVE_DISABLED_V1") && ttl == -1,
        "Account coordination metadata invalid; manual review required"
    );
    let value = |key: &str| values.get(key).map(String::as_str).unwrap_or("unknown");
    ensure!(
        value("state") == "Clean"
            && value("owner").is_empty()
            && value("unresolved") == "0"
            && value("position") == "0",
        "Account restart blocked: state={}, owner={}, unresolved={}, position={}. Review the retained run with native-kite-review before restarting; no new strategy owner acquired",
        value("state"),
        value("owner"),
        value("unresolved"),
        value("position")
    );
    Ok(())
}
impl Account {
    pub fn acquire(url: &str, account: &str, owner: &str) -> Result<Self> {
        ensure!(
            !owner.is_empty()
                && owner.len() <= 64
                && owner
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Invalid account owner"
        );
        let key = key(account)?;
        let mut connection = connection::connect(url)?;
        let script = "if redis.call('EXISTS',KEYS[1])==0 then redis.call('HSET',KEYS[1],'scope','NATIVE_DISABLED_V1','owner',ARGV[1],'state','Starting'); return 1 end; if redis.call('HGET',KEYS[1],'scope')~='NATIVE_DISABLED_V1' or redis.call('PTTL',KEYS[1])~=-1 or redis.call('HGET',KEYS[1],'state')~='Clean' or redis.call('HGET',KEYS[1],'owner')~='' then return 0 end; redis.call('HSET',KEYS[1],'owner',ARGV[1],'state','Starting'); return 2";
        let acquired: u32 = redis::cmd("EVAL")
            .arg(script)
            .arg(1)
            .arg(&key)
            .arg(owner)
            .query(&mut connection)
            .map_err(|_| anyhow!("Account acquisition uncertain; manual review required"))?;
        ensure!(
            acquired > 0,
            "Account already owned or requires restart review"
        );
        connection::sync(&mut connection)?;
        let scope = format!("native-account-{account}");
        // Conservative application budgets include cancellations and failed requests.
        let policy = Policy {
            per_second: 5,
            per_minute: 100,
            per_day: 1000,
        };
        let limiter = if acquired == 1 {
            Limiter::create_at(url, &scope, policy)?
        } else {
            Limiter::open_at(url, &scope, policy)?
        };
        Ok(Self {
            connection,
            key,
            owner: owner.into(),
            limiter,
            poisoned: false,
        })
    }
    pub fn update(&mut self, state: &str, unresolved: usize, position: i64) -> Result<()> {
        ensure!(
            !self.poisoned,
            "Account coordination requires manual review"
        );
        let result = (|| {
            let script = "if redis.call('HGET',KEYS[1],'owner')~=ARGV[1] or redis.call('HGET',KEYS[1],'scope')~='NATIVE_DISABLED_V1' or redis.call('PTTL',KEYS[1])~=-1 then return 0 end; if redis.call('HGET',KEYS[1],'state')=='ReviewRequired' and ARGV[2]~='ReviewRequired' then return 0 end; local t=redis.call('TIME'); redis.call('HSET',KEYS[1],'state',ARGV[2],'unresolved',ARGV[3],'position',ARGV[4],'heartbeat_ms',t[1]*1000+math.floor(t[2]/1000),'last_namespace',ARGV[1]); return 1";
            let ok: u32 = redis::cmd("EVAL")
                .arg(script)
                .arg(1)
                .arg(&self.key)
                .arg(&self.owner)
                .arg(state)
                .arg(unresolved)
                .arg(position)
                .query(&mut self.connection)
                .map_err(|_| anyhow!("Account heartbeat unavailable"))?;
            ensure!(ok == 1, "Account ownership lost; dispatch stopped");
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub fn reserve(&mut self) -> Result<()> {
        ensure!(!self.poisoned, "Account coordinator stopped");
        let ok:u32=redis::cmd("EVAL").arg("if redis.call('HGET',KEYS[1],'owner')~=ARGV[1] or redis.call('HGET',KEYS[1],'scope')~='NATIVE_DISABLED_V1' or redis.call('PTTL',KEYS[1])~=-1 or redis.call('HGET',KEYS[1],'state')=='ReviewRequired' then return 0 end; redis.call('HINCRBY',KEYS[1],'command_attempts',1); return 1").arg(1).arg(&self.key).arg(&self.owner).query(&mut self.connection).map_err(|_|anyhow!("Account admission unavailable"))?;
        ensure!(ok == 1, "Account ownership lost; no dispatch");
        ensure!(
            matches!(self.limiter.reserve()?, Decision::Allowed),
            "Account command rate budget exhausted; no dispatch"
        );
        Ok(())
    }
    pub fn cooldown(&mut self, ms: u64) -> Result<()> {
        self.limiter.cooldown(ms.clamp(10_000, 86_400_000))
    }
    pub fn finish(&mut self, clean: bool, unresolved: usize, position: i64) -> Result<()> {
        self.update(
            if clean { "Stopping" } else { "ReviewRequired" },
            unresolved,
            position,
        )?;
        if clean {
            let script = "if redis.call('HGET',KEYS[1],'owner')~=ARGV[1] or redis.call('HGET',KEYS[1],'state')~='Stopping' then return 0 end; redis.call('HSET',KEYS[1],'owner','','state','Clean'); return 1";
            let ok: u32 = redis::cmd("EVAL")
                .arg(script)
                .arg(1)
                .arg(&self.key)
                .arg(&self.owner)
                .query(&mut self.connection)
                .map_err(|_| anyhow!("Account release uncertain"))?;
            ensure!(ok == 1, "Account release ownership mismatch");
        }
        connection::sync(&mut self.connection)
    }
}
pub fn status(account: &str) -> Result<serde_json::Value> {
    status_at(&connection::url_from_env()?, account)
}
pub fn status_at(url: &str, account: &str) -> Result<serde_json::Value> {
    let mut connection = connection::connect(url)?;
    let values: BTreeMap<String, String> = redis::cmd("HGETALL")
        .arg(key(account)?)
        .query(&mut connection)
        .map_err(|_| anyhow!("Account health unavailable"))?;
    ensure!(
        values.get("scope").map(String::as_str) == Some("NATIVE_DISABLED_V1"),
        "Native account not found or invalid"
    );
    let (s, us): (u64, u64) = redis::cmd("TIME").query(&mut connection)?;
    let age = values
        .get("heartbeat_ms")
        .and_then(|v| v.parse::<u64>().ok())
        .map(|t| (s * 1000 + us / 1000).saturating_sub(t));
    Ok(
        serde_json::json!({"event":"native_kite_health","account":account,"state":values.get("state"),"owner":values.get("owner"),"last_namespace":values.get("last_namespace"),"unresolved":values.get("unresolved"),"position":values.get("position"),"heartbeat_age_ms":age,"stale":values.get("state").is_none_or(|s|s!="Clean") && age.is_none_or(|a|a>15000),"command_attempts":values.get("command_attempts"),"restart_blocked":values.get("state").is_none_or(|s|s!="Clean"),"requires_review":values.get("state").is_none_or(|s|s=="ReviewRequired" || (s!="Clean" && age.is_none_or(|a|a>15000))),"live_orders_enabled":false}),
    )
}

#[cfg(test)]
mod startup_tests {
    use super::*;
    #[test]
    fn admission_requires_absent_or_clean_flat_unowned_durable_account() {
        let empty = BTreeMap::new();
        assert!(validate_startup(&empty, -2).is_ok());
        assert!(validate_startup(&empty, -1).is_err());
        let clean: BTreeMap<String, String> = [
            ("scope", "NATIVE_DISABLED_V1"),
            ("state", "Clean"),
            ("owner", ""),
            ("unresolved", "0"),
            ("position", "0"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        assert!(validate_startup(&clean, -1).is_ok());
        assert!(validate_startup(&clean, 10_000).is_err());
        for (field, value) in [
            ("state", "ReviewRequired"),
            ("state", "Running"),
            ("owner", "previous-run"),
            ("unresolved", "1"),
            ("position", "-1"),
            ("scope", "unknown"),
        ] {
            let mut invalid = clean.clone();
            invalid.insert(field.into(), value.into());
            assert!(validate_startup(&invalid, -1).is_err());
        }
    }
}

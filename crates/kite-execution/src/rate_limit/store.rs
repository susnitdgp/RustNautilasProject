use super::policy::Policy;
use anyhow::{Result, anyhow, ensure};
use kite_journal::connection;
use serde::Serialize;
const SCRIPT: &str = include_str!("reserve.lua");
#[derive(Debug, Serialize, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    Deferred { retry_after_ms: u64 },
}
pub struct Limiter {
    connection: redis::Connection,
    key: String,
    policy: String,
    poisoned: bool,
}
impl Limiter {
    pub fn key(account_scope: &str) -> Result<String> {
        ensure!(
            !account_scope.is_empty()
                && account_scope.len() <= 64
                && account_scope
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "Invalid rate-limit account scope"
        );
        Ok(format!(
            "susanta:nautilus:sim:order-budget:{{{account_scope}}}"
        ))
    }
    pub fn create_at(url: &str, account_scope: &str, policy: Policy) -> Result<Self> {
        Self::load(url, account_scope, policy, true)
    }
    pub fn open_at(url: &str, account_scope: &str, policy: Policy) -> Result<Self> {
        Self::load(url, account_scope, policy, false)
    }
    fn load(url: &str, account_scope: &str, policy: Policy, create: bool) -> Result<Self> {
        policy.validate()?;
        let key = Self::key(account_scope)?;
        let policy = serde_json::to_string(&policy)?;
        let mut connection = connection::connect(url)?;
        if create {
            let initial=serde_json::json!({"version":1,"policy":policy,"history":[],"last_ms":0,"cooldown_until":0}).to_string();
            let created: Option<String> = redis::cmd("SET")
                .arg(&key)
                .arg(initial)
                .arg("NX")
                .query(&mut connection)
                .map_err(|_| anyhow!("Rate-limit initialization failed; outcome may be unknown"))?;
            ensure!(
                created.is_some(),
                "Rate-limit account scope already exists; reopen it"
            );
            connection::sync(&mut connection)?;
        } else {
            let valid:i32=redis::cmd("EVAL").arg("local r=redis.call('GET',KEYS[1]); if not r or #r>150000 or redis.call('PTTL',KEYS[1])~=-1 then return 0 end; local s=cjson.decode(r); if s.version~=1 or s.policy~=ARGV[1] then return 0 end; return 1")
                .arg(1).arg(&key).arg(&policy).query(&mut connection).map_err(|_|anyhow!("Rate-limit state unavailable"))?;
            ensure!(valid == 1, "Rate-limit account state or policy mismatch");
        }
        Ok(Self {
            connection,
            key,
            policy,
            poisoned: false,
        })
    }
    pub fn reserve(&mut self) -> Result<Decision> {
        self.execute("reserve", 0)
    }
    /// Applies a shared broker cooldown, never shortening an existing cooldown.
    pub fn cooldown(&mut self, milliseconds: u64) -> Result<()> {
        ensure!(
            (1..=86400000).contains(&milliseconds),
            "Cooldown must be 1 ms..24 hours"
        );
        self.execute("cooldown", milliseconds)?;
        Ok(())
    }
    fn execute(&mut self, operation: &str, milliseconds: u64) -> Result<Decision> {
        ensure!(
            !self.poisoned,
            "Rate-limit operation previously failed; reopen and reconcile"
        );
        let result = (|| -> Result<Decision> {
            let (allowed, retry): (u32, u64) = redis::cmd("EVAL")
                .arg(SCRIPT)
                .arg(1)
                .arg(&self.key)
                .arg(&self.policy)
                .arg(operation)
                .arg(milliseconds)
                .query(&mut self.connection)
                .map_err(|_| {
                    anyhow!("Redis rate-limit operation failed; outcome may be unknown")
                })?;
            ensure!(allowed <= 1, "Invalid rate-limit result");
            if allowed == 1 || operation == "cooldown" {
                connection::sync(&mut self.connection)?;
            }
            if allowed == 1 {
                Ok(Decision::Allowed)
            } else {
                Ok(Decision::Deferred {
                    retry_after_ms: retry,
                })
            }
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

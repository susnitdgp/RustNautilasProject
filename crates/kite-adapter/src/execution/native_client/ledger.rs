//! Native command ownership, persisted before any broker mutation.
use anyhow::{Result, anyhow, ensure};
use nautilus_model::events::OrderEventAny;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Record {
    pub events: Vec<OrderEventAny>,
    pub tag: String,
    pub product: String,
    pub token: u32,
    pub broker_id: Option<String>,
    pub outcome: String,
    pub management: BTreeMap<String, String>,
}
pub(crate) trait Store: Send {
    fn save(&mut self, id: &str, record: &Record) -> Result<()>;
    fn reserve(&mut self) -> Result<()> {
        Ok(())
    }
    fn cooldown(&mut self, _: u64) -> Result<()> {
        Ok(())
    }
    fn health(&mut self, _: &str, _: usize, _: i64) -> Result<()> {
        Ok(())
    }
    fn finish(&mut self, _: bool, _: usize, _: i64) -> Result<()> {
        Ok(())
    }
}
/// Retention of a run's command journal once the run ended clean (or, for earlier runs, once
/// a later run acquired the reviewed, Clean account lease). Unclean runs keep no TTL.
pub(crate) const FINISHED_JOURNAL_TTL_SECS: u64 = 30 * 24 * 3600;

/// Put the retention TTL on this slot's earlier run journals that still have none.
/// Housekeeping only: called after the lease was acquired from a Clean state, which means
/// every earlier run either finished clean or was reviewed. Returns the keys changed.
pub(crate) fn expire_previous_journals(
    connection: &mut redis::Connection,
    current_key: &str,
    namespace: &str,
) -> Result<usize> {
    let Some(at) = current_key.rfind(namespace) else {
        return Ok(0);
    };
    let pattern = format!("{}*", &current_key[..at]);
    let mut cursor = 0_u64;
    let mut changed = 0;
    loop {
        let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(&pattern)
            .arg("COUNT")
            .arg(500)
            .query(connection)
            .map_err(|_| anyhow!("Journal retention scan failed"))?;
        for key in keys.iter().filter(|k| k.as_str() != current_key) {
            // EXPIRE ... NX: only keys without a TTL; never shortens an existing one.
            let set: i32 = redis::cmd("EXPIRE")
                .arg(key)
                .arg(FINISHED_JOURNAL_TTL_SECS)
                .arg("NX")
                .query(connection)
                .map_err(|_| anyhow!("Journal retention update failed"))?;
            changed += set as usize;
        }
        if next == 0 {
            return Ok(changed);
        }
        cursor = next;
    }
}
pub(crate) struct RedisStore {
    account: Option<super::coordination::Account>,
    connection: redis::Connection,
    key: String,
    previous: BTreeMap<String, String>,
    poisoned: bool,
}
impl RedisStore {
    pub fn coordinated(keys: &super::keys::KeySpace, namespace: &str, account: &str) -> Result<Self> {
        let mut store = Self::create(keys, namespace)?;
        store.account = Some(super::coordination::Account::acquire_in(
            &kite_journal::connection::url_from_env()?,
            keys,
            account,
            namespace,
        )?);
        match expire_previous_journals(&mut store.connection, &store.key, namespace) {
            Ok(n) if n > 0 => eprintln!(
                "{}",
                serde_json::json!({"event":"native_journal_retention","expired_runs":n,"ttl_days":FINISHED_JOURNAL_TTL_SECS / 86400})
            ),
            Ok(_) => {}
            Err(e) => eprintln!("Journal retention skipped: {e:#}"),
        }
        Ok(store)
    }
    pub fn create(keys: &super::keys::KeySpace, namespace: &str) -> Result<Self> {
        let key = keys.commands(namespace)?;
        let mut connection =
            kite_journal::connection::connect(&kite_journal::connection::url_from_env()?)?;
        let created: bool = redis::cmd("HSETNX")
            .arg(&key)
            .arg("scope")
            .arg("NATIVE_KITE_DISABLED_V1")
            .query(&mut connection)
            .map_err(|_| anyhow!("Native Redis ownership initialization failed"))?;
        ensure!(
            created,
            "Native ownership namespace exists; manual recovery required"
        );
        kite_journal::connection::sync(&mut connection)?;
        Ok(Self {
            account: None,
            connection,
            key,
            previous: BTreeMap::new(),
            poisoned: false,
        })
    }
}
impl Store for RedisStore {
    fn reserve(&mut self) -> Result<()> {
        if let Some(a) = &mut self.account {
            a.reserve()?;
        }
        Ok(())
    }
    fn cooldown(&mut self, ms: u64) -> Result<()> {
        if let Some(a) = &mut self.account {
            a.cooldown(ms)?;
        }
        Ok(())
    }
    fn health(&mut self, s: &str, u: usize, p: i64) -> Result<()> {
        if let Some(a) = &mut self.account {
            a.update(s, u, p)?;
        }
        Ok(())
    }
    fn finish(&mut self, clean: bool, u: usize, p: i64) -> Result<()> {
        if let Some(a) = &mut self.account {
            a.finish(clean, u, p)?;
            if clean {
                // Clean run: keep the journal for review, then let Redis drop it.
                let set: redis::RedisResult<i32> = redis::cmd("EXPIRE")
                    .arg(&self.key)
                    .arg(FINISHED_JOURNAL_TTL_SECS)
                    .arg("NX")
                    .query(&mut self.connection);
                if set.is_err() {
                    eprintln!("Journal retention TTL not set; it will be set at the next start");
                }
            }
        }
        Ok(())
    }
    fn save(&mut self, id: &str, record: &Record) -> Result<()> {
        ensure!(
            !self.poisoned,
            "Native ownership persistence failed; reconcile before continuing"
        );
        let result = (|| -> Result<()> {
            ensure!(
                self.previous.len() < 10000 || self.previous.contains_key(id),
                "Native command journal full"
            );
            let value = serde_json::to_string(record)?;
            let script = "if redis.call('HGET',KEYS[1],'scope')~=ARGV[1] or redis.call('PTTL',KEYS[1])~=-1 then return 0 end; local old=redis.call('HGET',KEYS[1],ARGV[2]); if (old or '')~=ARGV[3] then return 0 end; local owner=redis.call('HGET',KEYS[1],ARGV[5]); if owner and owner~=ARGV[2] then return 0 end; redis.call('HSET',KEYS[1],ARGV[2],ARGV[4],ARGV[5],ARGV[2]); return 1";
            let ok: i32 = redis::cmd("EVAL")
                .arg(script)
                .arg(1)
                .arg(&self.key)
                .arg("NATIVE_KITE_DISABLED_V1")
                .arg(format!("order:{id}"))
                .arg(self.previous.get(id).map(String::as_str).unwrap_or(""))
                .arg(&value)
                .arg(format!("tag:{}", record.tag))
                .query(&mut self.connection)
                .map_err(|_| anyhow!("Native Redis write outcome unknown"))?;
            ensure!(
                ok == 1,
                "Native ownership changed; manual reconciliation required"
            );
            kite_journal::connection::sync(&mut self.connection)?;
            self.previous.insert(id.into(), value);
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

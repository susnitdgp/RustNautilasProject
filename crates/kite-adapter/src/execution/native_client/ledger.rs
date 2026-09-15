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
}
pub(crate) struct RedisStore {
    connection: redis::Connection,
    key: String,
    previous: BTreeMap<String, String>,
    poisoned: bool,
}
impl RedisStore {
    pub fn create(namespace: &str) -> Result<Self> {
        ensure!(
            !namespace.is_empty()
                && namespace.len() <= 64
                && namespace
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Invalid native command namespace"
        );
        let key = format!("susanta:nautilus:native-kite:commands:{{{namespace}}}");
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
            connection,
            key,
            previous: BTreeMap::new(),
            poisoned: false,
        })
    }
}
impl Store for RedisStore {
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

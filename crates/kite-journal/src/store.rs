use crate::{connection, model::Event, state::State};
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
const SCOPE: &str = "KITE_SIMULATION_ONLY_V2";
const CREATE: &str = r#"
if redis.call('EXISTS',KEYS[1])~=0 then return 0 end
redis.call('HSET',KEYS[1],'scope',ARGV[1],'generation',ARGV[2],'revision','0')
return 1
"#;
const SNAPSHOT: &str = r#"
if redis.call('TYPE',KEYS[1]).ok~='hash' then return redis.error_reply('invalid journal') end
if redis.call('HLEN',KEYS[1])>10003 or redis.call('PTTL',KEYS[1])~=-1 then return redis.error_reply('invalid journal') end
return redis.call('HGETALL',KEYS[1])
"#;
const APPEND: &str = r#"
if redis.call('TYPE',KEYS[1]).ok~='hash' then return 0 end
if redis.call('PTTL',KEYS[1])~=-1 then return 0 end
if redis.call('HGET',KEYS[1],'scope')~=ARGV[1] or redis.call('HGET',KEYS[1],'generation')~=ARGV[2] or redis.call('HGET',KEYS[1],'revision')~=ARGV[3] then return 0 end
if redis.call('HLEN',KEYS[1])~=tonumber(ARGV[3])+3 or redis.call('HEXISTS',KEYS[1],ARGV[4])~=0 then return 0 end
redis.call('HSET',KEYS[1],ARGV[4],ARGV[5],'revision',ARGV[6])
return 1
"#;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    recorded_at_ns: u64,
    event: Event,
}
pub struct Journal {
    connection: redis::Connection,
    key: String,
    generation: String,
    state: State,
    poisoned: bool,
    records: usize,
}
impl Journal {
    pub fn create(namespace: &str) -> Result<Self> {
        Self::create_at(&connection::url_from_env()?, namespace)
    }
    pub fn open(namespace: &str) -> Result<Self> {
        Self::open_at(&connection::url_from_env()?, namespace)
    }
    pub fn create_at(url: &str, namespace: &str) -> Result<Self> {
        Self::load(url, namespace, true)
    }
    pub fn open_at(url: &str, namespace: &str) -> Result<Self> {
        Self::load(url, namespace, false)
    }
    pub fn key(namespace: &str) -> Result<String> {
        ensure!(
            !namespace.is_empty()
                && namespace.len() <= 64
                && namespace
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "Journal namespace must be 1..64 letters, digits, hyphens or underscores"
        );
        Ok(format!("susanta:nautilus:sim:journal:{{{namespace}}}"))
    }
    fn load(url: &str, namespace: &str, create: bool) -> Result<Self> {
        let key = Self::key(namespace)?;
        let mut connection = connection::connect(url)?;
        if create {
            let generation = Uuid::new_v4().to_string();
            let created: i32 = redis::cmd("EVAL")
                .arg(CREATE)
                .arg(1)
                .arg(&key)
                .arg(SCOPE)
                .arg(generation)
                .query(&mut connection)
                .map_err(|_| anyhow!("Redis journal creation failed; outcome may be unknown"))?;
            ensure!(
                created == 1,
                "Journal namespace already exists; choose a new namespace or reopen"
            );
            connection::sync(&mut connection)?;
        }
        let mut values: BTreeMap<String, String> = redis::cmd("EVAL")
            .arg(SNAPSHOT)
            .arg(1)
            .arg(&key)
            .query(&mut connection)
            .map_err(|_| anyhow!("Redis journal snapshot unavailable or invalid"))?;
        ensure!(
            values.remove("scope").as_deref() == Some(SCOPE),
            "Journal scope mismatch"
        );
        let generation = values
            .remove("generation")
            .ok_or_else(|| anyhow!("Journal generation missing"))?;
        ensure!(
            Uuid::parse_str(&generation).is_ok(),
            "Invalid journal generation"
        );
        let records: usize = values
            .remove("revision")
            .ok_or_else(|| anyhow!("Journal revision missing"))?
            .parse()
            .map_err(|_| anyhow!("Invalid journal revision"))?;
        ensure!(
            records <= 10000 && values.len() == records,
            "Journal record count mismatch"
        );
        let mut state = State::default();
        for sequence in 1..=records {
            let payload = values
                .remove(&format!("event:{sequence}"))
                .ok_or_else(|| anyhow!("Journal sequence gap"))?;
            ensure!(payload.len() <= 8192, "Journal record too large");
            let record: Record =
                serde_json::from_str(&payload).map_err(|_| anyhow!("Invalid journal record"))?;
            ensure!(
                record.version == 2 && record.recorded_at_ns > 0,
                "Unsupported journal record"
            );
            ensure!(
                state.apply(&record.event)?,
                "Unexpected duplicate journal event"
            );
        }
        Ok(Self {
            connection,
            key,
            generation,
            state,
            poisoned: false,
            records,
        })
    }
    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn record_count(&self) -> usize {
        self.records
    }
    pub fn append(&mut self, event: Event) -> Result<bool> {
        ensure!(
            !self.poisoned,
            "Journal write previously failed; reopen and reconcile"
        );
        ensure!(self.records < 10000, "Journal record limit reached");
        let mut next = self.state.clone();
        if !next.apply(&event)? {
            return Ok(false);
        }
        let payload = serde_json::to_string(&Record {
            version: 2,
            recorded_at_ns: u64::try_from(
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            )?,
            event,
        })?;
        ensure!(payload.len() <= 8192, "Journal record too large");
        let result = (|| -> Result<()> {
            let changed: i32 = redis::cmd("EVAL")
                .arg(APPEND)
                .arg(1)
                .arg(&self.key)
                .arg(SCOPE)
                .arg(&self.generation)
                .arg(self.records)
                .arg(format!("event:{}", self.records + 1))
                .arg(payload)
                .arg(self.records + 1)
                .query(&mut self.connection)
                .map_err(|_| anyhow!("Redis journal write failed; outcome may be unknown"))?;
            ensure!(
                changed == 1,
                "Redis journal changed concurrently; reopen and reconcile"
            );
            connection::sync(&mut self.connection)
        })();
        if result.is_err() {
            self.poisoned = true;
            return result.map(|_| false);
        }
        self.state = next;
        self.records += 1;
        Ok(true)
    }
}

//! Append-only native events, committed before publication. Recovery is read-only.
use anyhow::{Result, anyhow, ensure};
use kite_journal::connection;
use nautilus_model::events::OrderEventAny;
pub struct Outbox {
    connection: redis::Connection,
    key: String,
    raw: String,
    events: Vec<OrderEventAny>,
    poisoned: bool,
}
impl Outbox {
    pub fn create_at(url: &str, namespace: &str) -> Result<Self> {
        kite_journal::store::Journal::key(namespace)?;
        let key = format!("susanta:nautilus:sim:native-events:{{{namespace}}}");
        let mut connection = connection::connect(url)?;
        let created: Option<String> = redis::cmd("SET")
            .arg(&key)
            .arg("[]")
            .arg("NX")
            .query(&mut connection)
            .map_err(|_| anyhow!("Native outbox create failed"))?;
        ensure!(created.is_some(), "Native namespace already exists");
        connection::sync(&mut connection)?;
        Ok(Self {
            connection,
            key,
            raw: "[]".into(),
            events: vec![],
            poisoned: false,
        })
    }
    pub fn append(&mut self, batch: &[OrderEventAny]) -> Result<()> {
        ensure!(
            !self.poisoned,
            "Native outbox uncertain; reconciliation required"
        );
        ensure!(
            self.events.len() + batch.len() <= 10000,
            "Native simulation event limit exceeded"
        );
        let mut next = self.events.clone();
        next.extend_from_slice(batch);
        let raw = serde_json::to_string(&next)?;
        let result = (|| {
            let changed:i32=redis::cmd("EVAL").arg("if redis.call('GET',KEYS[1])~=ARGV[1] or redis.call('PTTL',KEYS[1])~=-1 then return 0 end; redis.call('SET',KEYS[1],ARGV[2]);return 1").arg(1).arg(&self.key).arg(&self.raw).arg(&raw).query(&mut self.connection).map_err(|_|anyhow!("Native event write uncertain"))?;
            ensure!(changed == 1, "Native event writer conflict");
            connection::sync(&mut self.connection)
        })();
        if result.is_err() {
            self.poisoned = true;
        } else {
            self.raw = raw;
            self.events = next;
        }
        result
    }
    pub fn read_at(url: &str, namespace: &str) -> Result<Vec<OrderEventAny>> {
        kite_journal::store::Journal::key(namespace)?;
        let mut connection = connection::connect(url)?;
        let raw: String = redis::cmd("GET")
            .arg(format!(
                "susanta:nautilus:sim:native-events:{{{namespace}}}"
            ))
            .query(&mut connection)
            .map_err(|_| anyhow!("Native event read failed"))?;
        ensure!(
            raw.len() <= 32 * 1024 * 1024,
            "Native event data exceeds bound"
        );
        let events: Vec<OrderEventAny> =
            serde_json::from_str(&raw).map_err(|_| anyhow!("Invalid native events"))?;
        ensure!(events.len() <= 10000, "Native event count exceeds bound");
        Ok(events)
    }
}

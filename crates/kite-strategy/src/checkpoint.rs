use crate::crossover::Strategy;
use anyhow::{Result, anyhow, ensure};
use kite_journal::connection;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub version: u32,
    pub cursor: usize,
    pub strategy: Strategy,
    pub finished: bool,
}
pub struct Store {
    connection: redis::Connection,
    key: String,
    revision: u64,
    poisoned: bool,
}
impl Store {
    pub fn create_at(url: &str, namespace: &str) -> Result<Self> {
        kite_journal::store::Journal::key(namespace)?;
        let key = format!("susanta:nautilus:sim:strategy:{{{namespace}}}");
        let mut connection = connection::connect(url)?;
        let result: Option<String> = redis::cmd("SET")
            .arg(&key)
            .arg("0")
            .arg("NX")
            .query(&mut connection)
            .map_err(|_| anyhow!("Strategy checkpoint creation failed"))?;
        ensure!(result.is_some(), "Strategy namespace already exists");
        connection::sync(&mut connection)?;
        Ok(Self {
            connection,
            key,
            revision: 0,
            poisoned: false,
        })
    }
    pub fn save(&mut self, value: &Snapshot) -> Result<()> {
        ensure!(
            !self.poisoned,
            "Strategy checkpoint failed; restart requires reconciliation"
        );
        let payload = serde_json::to_string(value)?;
        let result = (|| -> Result<()> {
            let changed:i32=redis::cmd("EVAL").arg("local old=redis.call('GET',KEYS[1]); if not old or redis.call('PTTL',KEYS[1])~=-1 then return 0 end; local v=0; if old~='0' then v=cjson.decode(old).revision end; if v~=tonumber(ARGV[1]) then return 0 end; redis.call('SET',KEYS[1],ARGV[2]);return 1")
                .arg(1).arg(&self.key).arg(self.revision).arg(serde_json::json!({"revision":self.revision+1,"snapshot":serde_json::from_str::<serde_json::Value>(&payload)?}).to_string()).query(&mut self.connection).map_err(|_|anyhow!("Strategy checkpoint write uncertain"))?;
            ensure!(changed == 1, "Strategy checkpoint changed concurrently");
            connection::sync(&mut self.connection)
        })();
        if result.is_err() {
            self.poisoned = true;
        } else {
            self.revision += 1;
        }
        result
    }
    pub fn read(&mut self) -> Result<Snapshot> {
        let raw: String = redis::cmd("GET")
            .arg(&self.key)
            .query(&mut self.connection)
            .map_err(|_| anyhow!("Strategy checkpoint read failed"))?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| anyhow!("Invalid strategy checkpoint"))?;
        serde_json::from_value(value["snapshot"].clone())
            .map_err(|_| anyhow!("Invalid strategy snapshot"))
    }
}

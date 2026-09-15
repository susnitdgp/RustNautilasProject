use anyhow::{Result, anyhow, ensure};
use kite_journal::connection;
pub struct ControlStore {
    connection: redis::Connection,
    key: String,
    raw: String,
}
impl ControlStore {
    pub fn create_at(url: &str, namespace: &str) -> Result<Self> {
        kite_journal::store::Journal::key(namespace)?;
        let key = format!("susanta:nautilus:sim:session:{{{namespace}}}");
        let mut connection = connection::connect(url)?;
        let made: Option<String> = redis::cmd("SET")
            .arg(&key)
            .arg("null")
            .arg("NX")
            .query(&mut connection)
            .map_err(|_| anyhow!("Session creation failed"))?;
        ensure!(made.is_some(), "Paper session already exists");
        connection::sync(&mut connection)?;
        Ok(Self {
            connection,
            key,
            raw: "null".into(),
        })
    }
    pub fn save(&mut self, value: serde_json::Value) -> Result<()> {
        let raw = serde_json::to_string(&value)?;
        let result:i32=redis::cmd("EVAL").arg("if redis.call('GET',KEYS[1])~=ARGV[1] or redis.call('PTTL',KEYS[1])~=-1 then return 0 end; redis.call('SET',KEYS[1],ARGV[2]);return 1").arg(1).arg(&self.key).arg(&self.raw).arg(&raw).query(&mut self.connection).map_err(|_|anyhow!("Paper session write uncertain"))?;
        ensure!(result == 1, "Paper session writer conflict");
        connection::sync(&mut self.connection)?;
        self.raw = raw;
        Ok(())
    }
    pub fn read_at(url: &str, namespace: &str) -> Result<serde_json::Value> {
        kite_journal::store::Journal::key(namespace)?;
        let mut connection = connection::connect(url)?;
        let raw: String = redis::cmd("GET")
            .arg(format!("susanta:nautilus:sim:session:{{{namespace}}}"))
            .query(&mut connection)
            .map_err(|_| anyhow!("Paper session read failed"))?;
        serde_json::from_str(&raw).map_err(|_| anyhow!("Invalid paper session"))
    }
}

//! Durable Redis compare-and-swap state for ILRC lifecycle transitions.
//! Uses an exclusive namespace; unknown Redis outcomes fail closed. No broker calls.
use super::{ilrc_orchestrator::Orchestrator, ilrc_order_lifecycle::Lifecycle};
use anyhow::{Result, ensure};
use serde::Serialize;

pub struct Journal {
    connection: redis::Connection,
    key: String,
    revision: u64,
    poisoned: bool,
}
impl Journal {
    pub fn create(namespace: &str, initial: &impl Serialize) -> Result<Self> {
        ensure!(
            !namespace.is_empty()
                && namespace.len() <= 64
                && namespace
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Invalid ILRC namespace"
        );
        let url = kite_journal::connection::url_from_env()?;
        let mut connection = kite_journal::connection::connect(&url)?;
        let key = format!("ilrc:execution:journal:{{{namespace}}}");
        let initial = serde_json::to_string(initial)?;
        let created:i64=redis::cmd("EVAL").arg("if redis.call('EXISTS',KEYS[1])~=0 then return 0 end; redis.call('HSET',KEYS[1],'rev','0','state',ARGV[1]); return 1").arg(1).arg(&key).arg(initial).query(&mut connection)?;
        ensure!(
            created == 1,
            "Existing ILRC execution journal; manual recovery required"
        );
        kite_journal::connection::sync(&mut connection)?;
        Ok(Self {
            connection,
            key,
            revision: 0,
            poisoned: false,
        })
    }
    pub fn transition(&mut self, expected: &impl Serialize, next: &impl Serialize) -> Result<()> {
        ensure!(
            !self.poisoned,
            "ILRC journal must be reconciled before another write"
        );
        let prev = serde_json::to_string(expected)?;
        let value = serde_json::to_string(next)?;
        let script = "if redis.call('HGET',KEYS[1],'rev')~=ARGV[1] or redis.call('HGET',KEYS[1],'state')~=ARGV[2] or redis.call('PTTL',KEYS[1])~=-1 then return 0 end; redis.call('HSET',KEYS[1],'rev',ARGV[3],'state',ARGV[4]); return 1";
        let result: Result<()> = (|| {
            let updated: i64 = redis::cmd("EVAL")
                .arg(script)
                .arg(1)
                .arg(&self.key)
                .arg(self.revision.to_string())
                .arg(prev)
                .arg((self.revision + 1).to_string())
                .arg(value)
                .query(&mut self.connection)?;
            ensure!(updated == 1, "ILRC journal conflict; freeze and reconcile");
            kite_journal::connection::sync(&mut self.connection)?;
            self.revision += 1;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}
/// Writes lifecycle transitions before allowing callers to perform another action.
/// Broker mutation remains outside this type and must not occur until reservation succeeds.
pub struct PersistentState {
    pub lifecycle: Lifecycle,
    journal: Journal,
}
impl PersistentState {
    pub fn create(namespace: &str) -> Result<Self> {
        Ok(Self {
            lifecycle: Lifecycle::default(),
            journal: Journal::create(namespace, &Lifecycle::default())?,
        })
    }
    pub fn transition(&mut self, update: impl FnOnce(&mut Lifecycle) -> Result<()>) -> Result<()> {
        let mut next = self.lifecycle.clone();
        update(&mut next)?;
        self.journal.transition(&self.lifecycle, &next)?;
        self.lifecycle = next;
        Ok(())
    }
}
/// Complete ILRC orchestration state is journaled, including stop ID and trigger.
/// A previous namespace is never resumed automatically; manual broker review is mandatory.
pub struct PersistentOrchestrator {
    pub orchestration: Orchestrator,
    journal: Journal,
}
impl PersistentOrchestrator {
    pub fn create(namespace: &str) -> Result<Self> {
        let orchestration = Orchestrator::default();
        let journal = Journal::create(namespace, &orchestration)?;
        Ok(Self {
            orchestration,
            journal,
        })
    }
    pub fn transition<T>(&mut self, op: impl FnOnce(&mut Orchestrator) -> Result<T>) -> Result<T> {
        let mut candidate = self.orchestration.clone();
        let outcome = op(&mut candidate)?;
        self.journal.transition(&self.orchestration, &candidate)?;
        self.orchestration = candidate;
        Ok(outcome)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_state_is_serializable_and_review_required_is_retained() {
        let mut s = Lifecycle::default();
        s.reserve("test-1", 1).unwrap();
        let restored: Lifecycle = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert!(!restored.can_enter());
    }
    #[test]
    fn orchestrator_checkpoint_preserves_stop_identity() {
        let o = Orchestrator {
            protective_order_id: Some("123456".into()),
            stop_trigger: 8700,
            ..Orchestrator::default()
        };
        let replay: Orchestrator =
            serde_json::from_slice(&serde_json::to_vec(&o).unwrap()).unwrap();
        assert_eq!(replay.protective_order_id.as_deref(), Some("123456"));
        assert_eq!(replay.stop_trigger, 8700);
    }
}

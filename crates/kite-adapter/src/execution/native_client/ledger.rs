//! Native order records for one run (kite-adapter 0.4.0). They live in the dispatcher's
//! memory only: no journal, no crash recovery. Kite is the source of truth; after a crash
//! the next run starts from a flat account with no open orders.
use anyhow::Result;
use nautilus_model::events::OrderEventAny;
use std::collections::BTreeMap;
#[derive(Clone, Debug)]
pub(crate) struct Record {
    pub events: Vec<OrderEventAny>,
    pub tag: String,
    pub product: String,
    pub token: u32,
    pub broker_id: Option<String>,
    pub outcome: String,
    pub management: BTreeMap<String, String>,
}
/// What the dispatcher needs around a broker command. `save` is a hook only: production
/// keeps nothing (tests use it to observe that a record changes before the broker call).
pub(crate) trait Store: Send {
    fn save(&mut self, _id: &str, _record: &Record) -> Result<()> {
        Ok(())
    }
    /// Take one slot from the shared order-rate budget before a broker command.
    fn reserve(&mut self) -> Result<()> {
        Ok(())
    }
    /// Kite rate-limited a request: pause the shared budget.
    fn cooldown(&mut self, _: u64) -> Result<()> {
        Ok(())
    }
}
/// Production/paper store: nothing persisted, only the shared Redis order-rate budget.
pub(crate) struct RunStore {
    budget: super::coordination::OrderBudget,
}
impl RunStore {
    pub fn open(keys: &super::keys::KeySpace, account: &str) -> Result<Self> {
        Ok(Self {
            budget: super::coordination::OrderBudget::open(keys, account)?,
        })
    }
}
impl Store for RunStore {
    fn reserve(&mut self) -> Result<()> {
        self.budget.reserve()
    }
    fn cooldown(&mut self, ms: u64) -> Result<()> {
        self.budget.cooldown(ms)
    }
}

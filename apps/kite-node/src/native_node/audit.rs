use anyhow::Result;
use nautilus_common::{
    actor::{DataActor, DataActorCore},
    nautilus_actor,
};
use nautilus_model::{data::QuoteTick, identifiers::InstrumentId};
use std::{cell::Cell, rc::Rc};
#[derive(Debug)]
pub struct AuditActor {
    core: DataActorCore,
    instrument: InstrumentId,
    count: Rc<Cell<u64>>,
}
impl AuditActor {
    pub fn new(instrument: InstrumentId, count: Rc<Cell<u64>>) -> Self {
        Self {
            core: DataActorCore::new(Default::default()),
            instrument,
            count,
        }
    }
}
nautilus_actor!(AuditActor);
impl DataActor for AuditActor {
    fn on_start(&mut self) -> Result<()> {
        self.subscribe_quotes(self.instrument, None, None);
        Ok(())
    }
    fn on_stop(&mut self) -> Result<()> {
        Ok(())
    }
    fn on_quote(&mut self, _: &QuoteTick) -> Result<()> {
        self.count.set(self.count.get() + 1);
        Ok(())
    }
}

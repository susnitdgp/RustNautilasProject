//! Kite full packets as native Nautilus custom data. Depth is a snapshot, not exchange deltas.
use crate::mapping::market_data::Snapshot;
use nautilus_core::UnixNanos;
use nautilus_model::data::{CustomDataTrait, HasTsInit, QuoteTick};
use serde::{Deserialize, Serialize};
use std::{any::Any, sync::Arc};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KiteFullTick {
    pub snapshot: Snapshot,
    pub quote: QuoteTick,
}
impl HasTsInit for KiteFullTick {
    fn ts_init(&self) -> UnixNanos {
        self.quote.ts_init
    }
}
impl CustomDataTrait for KiteFullTick {
    fn type_name(&self) -> &'static str {
        "KiteFullTick"
    }
    fn type_name_static() -> &'static str {
        "KiteFullTick"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ts_event(&self) -> UnixNanos {
        self.quote.ts_event
    }
    fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(self)?)
    }
    fn clone_arc(&self) -> Arc<dyn CustomDataTrait> {
        Arc::new(self.clone())
    }
    fn eq_arc(&self, other: &dyn CustomDataTrait) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| o == self)
    }
    fn from_json(value: serde_json::Value) -> anyhow::Result<Arc<dyn CustomDataTrait>> {
        Ok(Arc::new(serde_json::from_value::<Self>(value)?))
    }
}

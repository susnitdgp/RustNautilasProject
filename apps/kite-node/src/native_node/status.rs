use nautilus_core::UnixNanos;
use nautilus_model::data::{CustomDataTrait, HasTsInit};
use serde::{Deserialize, Serialize};
use std::{any::Any, sync::Arc};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedStatus {
    pub kind: String,
    pub generation: u32,
    pub ts: UnixNanos,
}
impl HasTsInit for FeedStatus {
    fn ts_init(&self) -> UnixNanos {
        self.ts
    }
}
impl CustomDataTrait for FeedStatus {
    fn type_name(&self) -> &'static str {
        "KiteFeedStatus"
    }
    fn type_name_static() -> &'static str {
        "KiteFeedStatus"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn ts_event(&self) -> UnixNanos {
        self.ts
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
    fn from_json(v: serde_json::Value) -> anyhow::Result<Arc<dyn CustomDataTrait>> {
        Ok(Arc::new(serde_json::from_value::<Self>(v)?))
    }
}

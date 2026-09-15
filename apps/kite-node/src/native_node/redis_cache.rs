//! Factory for the native Nautilus Redis adapter. No event filtering or local journal.
use nautilus_common::cache::{
    CacheConfig,
    database::{CacheDatabaseAdapter, CacheDatabaseFactory},
};
use nautilus_core::UUID4;
use nautilus_infrastructure::redis::cache::{
    RedisCacheConfig, RedisCacheDatabase, RedisCacheDatabaseAdapter,
};
use nautilus_model::identifiers::TraderId;
#[derive(Debug)]
pub struct Factory(pub RedisCacheConfig);
#[async_trait::async_trait]
impl CacheDatabaseFactory for Factory {
    async fn create(
        &self,
        trader_id: TraderId,
        instance_id: UUID4,
        config: CacheConfig,
    ) -> anyhow::Result<Box<dyn CacheDatabaseAdapter>> {
        let database =
            RedisCacheDatabase::new(trader_id, instance_id, config, self.0.clone()).await?;
        Ok(Box::new(RedisCacheDatabaseAdapter { database }))
    }
}

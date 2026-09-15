use crate::data::{client::KiteDataClient, config::KiteDataClientConfig};
use anyhow::{Result, anyhow};
use nautilus_common::{
    cache::CacheView,
    clients::DataClient,
    clock::Clock,
    factories::{ClientConfig, DataClientFactory},
};
use std::{cell::RefCell, rc::Rc};

#[derive(Debug)]
pub struct KiteDataClientFactory;

impl DataClientFactory for KiteDataClientFactory {
    fn create(
        &self,
        name: &str,
        config: &dyn ClientConfig,
        cache: CacheView,
        _clock: Rc<RefCell<dyn Clock>>,
    ) -> Result<Box<dyn DataClient>> {
        let config = config
            .as_any()
            .downcast_ref::<KiteDataClientConfig>()
            .ok_or_else(|| anyhow!("Kite factory requires KiteDataClientConfig"))?;
        Ok(Box::new(KiteDataClient::new(name, config.clone(), cache)?))
    }
    fn name(&self) -> &str {
        "KITE"
    }
    fn config_type(&self) -> &str {
        "KiteDataClientConfig"
    }
}

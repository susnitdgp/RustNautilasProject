//! Explicit native production dispatcher. Disabled unless both gates are enabled.
use super::{Client, Config, broker::KiteBroker, dispatch::Dispatcher, ledger::RedisStore};
use anyhow::{Result, anyhow, ensure};
use nautilus_common::{
    cache::CacheView,
    clients::ExecutionClient,
    factories::{ClientConfig, ExecutionClientFactory},
};
use nautilus_model::identifiers::TraderId;
use serde::Deserialize;
use std::{
    any::Any,
    sync::{Arc, atomic::AtomicBool},
};
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub expected_user_id: String,
    pub product: String,
    pub instrument_token: u32,
    pub live_orders_enabled: bool,
    pub market_protection: i32,
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.market_protection == -1,
            "Reviewed production policy requires automatic market protection (-1)"
        );
        ensure!(
            cfg!(feature = "live-orders"),
            "Production execution requires a separately reviewed live-orders build"
        );
        ensure!(
            self.live_orders_enabled,
            "Production order submission is disabled in broker settings"
        );
        ensure!(
            !self.expected_user_id.is_empty()
                && self.expected_user_id.len() <= 28
                && self.expected_user_id != "REPLACE_ME"
                && self
                    .expected_user_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric()),
            "Set the exact reviewed Kite user ID"
        );
        ensure!(
            self.product == "MIS" && self.instrument_token == 144870151,
            "Production scope is September CRUDEOIL MIS only"
        );
        Ok(())
    }
}
#[derive(Debug)]
pub struct LiveConfig {
    pub settings: Settings,
    pub namespace: String,
    pub stop_signal: Arc<AtomicBool>,
}
impl ClientConfig for LiveConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
#[derive(Debug)]
pub struct Factory;
impl ExecutionClientFactory for Factory {
    fn name(&self) -> &str {
        "KITE-PRODUCTION"
    }
    fn config_type(&self) -> &str {
        "KiteProductionConfig"
    }
    fn create(
        &self,
        trader: TraderId,
        name: &str,
        config: &dyn ClientConfig,
        cache: CacheView,
    ) -> Result<Box<dyn ExecutionClient>> {
        let c = config
            .as_any()
            .downcast_ref::<LiveConfig>()
            .ok_or_else(|| anyhow!("Wrong production configuration"))?;
        c.settings.validate()?;
        let credentials = Arc::new(crate::credentials::redis::load_from_env()?);
        let broker = KiteBroker::new(
            &credentials,
            c.settings.expected_user_id.clone(),
            c.settings.product.clone(),
        )?;
        let mut client = Client::new(
            trader,
            name,
            Config {
                user_id: c.settings.expected_user_id.clone(),
                product: c.settings.product.clone(),
                instrument_token: c.settings.instrument_token,
                credentials,
            },
            Box::new(broker),
        )?;
        client.dispatcher = Some(Arc::new(tokio::sync::Mutex::new(Dispatcher::new(
            client.broker.clone(),
            Box::new(RedisStore::coordinated(
                &c.namespace,
                &c.settings.expected_user_id,
            )?),
            client.factory.clone(),
            c.settings.product.clone(),
            c.settings.instrument_token,
        ))));
        client.cache = Some(cache);
        client.stop_signal = Some(c.stop_signal.clone());
        client.production = true;
        Ok(Box::new(client))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_configuration_never_enables_broker_mutation() {
        let mut s = Settings {
            expected_user_id: "TEST123".into(),
            product: "MIS".into(),
            instrument_token: 144870151,
            live_orders_enabled: false,
            market_protection: -1,
        };
        assert!(s.validate().is_err());
        s.live_orders_enabled = true;
        assert_eq!(s.validate().is_ok(), cfg!(feature = "live-orders"));
        s.product = "NRML".into();
        assert!(s.validate().is_err());
        s.product = "MIS".into();
        s.expected_user_id = "REPLACE_ME".into();
        assert!(s.validate().is_err());
    }
}

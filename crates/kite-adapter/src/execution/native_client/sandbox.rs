//! Official Kite sandbox only. Fixed hosts and sandbox-only Redis credential keys.
use super::{Client, Config, broker::KiteBroker, dispatch::Dispatcher, ledger::RedisStore};
use crate::{
    credentials::{KiteCredentials, redis::load_sandbox},
    http::authenticated::{Endpoint, ReadClient},
};
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
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub expected_user_id: String,
    pub product: String,
    pub instrument_token: u32,
    pub seconds: u64,
}
impl Settings {
    pub fn parse(text: &str) -> Result<Self> {
        let s: Self = toml::from_str(text)?;
        ensure!(
            s.expected_user_id != "REPLACE_WITH_SANDBOX_USER_ID"
                && s.expected_user_id.len() <= 28
                && !s.expected_user_id.is_empty()
                && s.expected_user_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric()),
            "Set expected sandbox user ID in config/kite-sandbox.toml"
        );
        ensure!(
            matches!(s.product.as_str(), "NRML" | "MIS")
                && s.instrument_token == 144870151
                && (1..=300).contains(&s.seconds),
            "Unsupported sandbox configuration"
        );
        Ok(s)
    }
    pub fn account_scope(&self) -> String {
        format!("SB{}", self.expected_user_id)
    }
}
#[derive(Deserialize)]
struct Profile {
    user_id: String,
    exchanges: Vec<String>,
    products: Vec<String>,
}
pub async fn preflight() -> Result<serde_json::Value> {
    let credentials = load_sandbox()?;
    let read = ReadClient::sandbox(&credentials)?;
    let p: Profile = read.get(Endpoint::Profile).await?;
    let quotes: serde_json::Value = read.sandbox_quote().await?;
    Ok(
        serde_json::json!({"event":"kite_sandbox_preflight","user_id":p.user_id,"mcx_enabled":p.exchanges.iter().any(|e|e=="MCX"),"products":p.products,"contract_available":quotes.get("MCX:CRUDEOIL26SEPFUT").is_some(),"instrument_token":quotes["MCX:CRUDEOIL26SEPFUT"]["instrument_token"],"live_orders_enabled":false,"orders_sent":0}),
    )
}
pub async fn prepare(settings: &Settings) -> Result<Arc<KiteCredentials>> {
    let credentials = Arc::new(load_sandbox()?);
    let read = ReadClient::sandbox(&credentials)?;
    let p: Profile = read.get(Endpoint::Profile).await?;
    ensure!(
        p.user_id == settings.expected_user_id
            && p.exchanges.iter().any(|e| e == "MCX")
            && p.products.contains(&settings.product),
        "Sandbox account identity or permission mismatch"
    );
    let quotes: serde_json::Value = read.sandbox_quote().await?;
    ensure!(
        quotes["MCX:CRUDEOIL26SEPFUT"]["instrument_token"].as_u64()
            == Some(u64::from(settings.instrument_token)),
        "CRUDEOIL sandbox contract unavailable or token mismatch"
    );
    Ok(credentials)
}
#[derive(Debug)]
pub struct SandboxConfig {
    pub namespace: String,
    pub user_id: String,
    pub product: String,
    pub instrument_token: u32,
    pub stop_signal: Arc<AtomicBool>,
}
impl ClientConfig for SandboxConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
#[derive(Debug, Default)]
pub struct SandboxFactory;
impl ExecutionClientFactory for SandboxFactory {
    fn name(&self) -> &str {
        "KITE-SANDBOX"
    }
    fn config_type(&self) -> &str {
        "KiteSandboxExecutionConfig"
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
            .downcast_ref::<SandboxConfig>()
            .ok_or_else(|| anyhow!("Invalid sandbox client config"))?;
        let credentials = Arc::new(load_sandbox()?);
        let broker = KiteBroker::sandbox(&credentials, c.user_id.clone(), c.product.clone())?;
        let mut client = Client::new(
            trader,
            name,
            Config {
                user_id: c.user_id.clone(),
                product: c.product.clone(),
                instrument_token: c.instrument_token,
                credentials,
            },
            Box::new(broker),
        )?;
        client.dispatcher = Some(Arc::new(tokio::sync::Mutex::new(Dispatcher::new(
            client.broker.clone(),
            Box::new(RedisStore::coordinated(
                &c.namespace,
                &format!("SB{}", c.user_id),
            )?),
            client.factory.clone(),
            c.product.clone(),
            c.instrument_token,
        ))));
        client.cache = Some(cache);
        client.stop_signal = Some(c.stop_signal.clone());
        Ok(Box::new(client))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sandbox_configuration_rejects_unknown_hosts_and_unverified_contracts() {
        let s = "expected_user_id='SBX123'\nproduct='NRML'\ninstrument_token=144870151\nseconds=60";
        assert!(Settings::parse(s).is_ok());
        assert!(Settings::parse(&format!("{s}\nroot='https://api.kite.trade'")).is_err());
        assert!(Settings::parse(&s.replace("144870151", "123")).is_err());
        assert!(Settings::parse(&s.replace("SBX123", "REPLACE_WITH_SANDBOX_USER_ID")).is_err());
    }
}

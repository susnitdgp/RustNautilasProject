//! Deterministic Kite protocol fixture. It cannot access credentials or a network.
use super::super::{
    broker_events::{BrokerOrder, BrokerTrade},
    request::Command,
    transport::Outcome,
};
use super::{
    Client, Config,
    broker::{Broker, BrokerPosition, Funds, Snapshot, Utilised},
    dispatch::Dispatcher,
    ledger::RedisStore,
};
use anyhow::{Result, anyhow, ensure};
use async_trait::async_trait;
use nautilus_common::{
    cache::CacheView,
    clients::ExecutionClient,
    factories::{ClientConfig, ExecutionClientFactory},
};
use nautilus_model::identifiers::TraderId;
use rust_decimal::Decimal;
use std::{
    any::Any,
    sync::{Arc, Mutex},
};
#[derive(Debug)]
pub struct MockConfig {
    pub namespace: String,
    pub stop_signal: Arc<std::sync::atomic::AtomicBool>,
    pub product: String,
    pub instrument_token: u32,
}
impl ClientConfig for MockConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
#[derive(Debug, Default)]
pub struct MockFactory;
impl ExecutionClientFactory for MockFactory {
    fn name(&self) -> &str {
        "KITE-MOCK"
    }
    fn config_type(&self) -> &str {
        "KiteNativeMockConfig"
    }
    fn create(
        &self,
        trader: TraderId,
        name: &str,
        config: &dyn ClientConfig,
        cache: CacheView,
    ) -> Result<Box<dyn ExecutionClient>> {
        let cfg = config
            .as_any()
            .downcast_ref::<MockConfig>()
            .ok_or_else(|| anyhow!("Invalid native Kite mock config"))?;
        let config = Config {
            user_id: "MOCK".into(),
            product: cfg.product.clone(),
            instrument_token: cfg.instrument_token,
            credentials: Arc::new(crate::credentials::KiteCredentials::new(
                Some("MOCKONLY".into()),
                Some("MOCKONLY".into()),
            )?),
        };
        let mut client = Client::new(
            trader,
            name,
            config,
            Box::new(MockBroker::new(cfg.instrument_token, &cfg.product)),
        )?;
        client.dispatcher = Some(Arc::new(tokio::sync::Mutex::new(Dispatcher::new(
            client.broker.clone(),
            Box::new(RedisStore::coordinated(&cfg.namespace, "MOCK")?),
            client.factory.clone(),
            cfg.product.clone(),
            cfg.instrument_token,
        ))));
        client.cache = Some(cache);
        client.stop_signal = Some(cfg.stop_signal.clone());
        Ok(Box::new(client))
    }
}
pub(crate) struct MockBroker {
    state: Mutex<Snapshot>,
    delay: std::sync::atomic::AtomicUsize,
    token: u32,
    product: String,
}
impl MockBroker {
    #[cfg(test)]
    pub fn delayed(self, polls: usize) -> Self {
        self.delay.store(polls, std::sync::atomic::Ordering::SeqCst);
        self
    }

    pub fn new(token: u32, product: &str) -> Self {
        Self {
            delay: std::sync::atomic::AtomicUsize::new(0),
            state: Mutex::new(Snapshot {
                orders: vec![],
                trades: vec![],
                positions: vec![],
                funds: Funds {
                    ledger: Some("mock"),
                    enabled: true,
                    net: Decimal::from(1_000_000),
                    utilised: Utilised {
                        debits: Decimal::ZERO,
                    },
                },
            }),
            token,
            product: product.into(),
        }
    }
    fn time() -> String {
        chrono::Utc::now()
            .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("India offset"))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }
}
#[async_trait]
impl Broker for MockBroker {
    async fn fees(
        &self,
        snapshot: &Snapshot,
        product: &str,
        token: u32,
    ) -> Result<super::fees::Fees> {
        let groups = super::fees::groups(snapshot, product, token)?;
        let mut fees = super::fees::Fees::new();
        for trades in groups.values() {
            fees.extend(super::fees::allocate(Decimal::ZERO, trades)?);
        }
        Ok(fees)
    }

    async fn verify(&self) -> Result<()> {
        Ok(())
    }
    async fn snapshot(&self) -> Result<Snapshot> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| anyhow!("Mock broker state unavailable"))?;
        if s.orders.iter().any(|o| o.status == "OPEN")
            && self
                .delay
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |n| n.checked_sub(1),
                )
                .is_ok()
        {
            return Ok(s.clone());
        }
        let pending: Vec<_> = s
            .orders
            .iter()
            .filter(|o| o.status == "OPEN")
            .cloned()
            .collect();
        for o in pending {
            let side = if o.transaction_type == "BUY" { 1 } else { -1 };
            let previous = s.positions.first().map_or(0, |p| p.quantity);
            let qty = previous + side * i64::from(o.quantity);
            let timestamp = Self::time();
            s.trades.push(BrokerTrade {
                trade_id: o.order_id.clone(),
                order_id: o.order_id.clone(),
                exchange: "MCX".into(),
                tradingsymbol: o.tradingsymbol.clone(),
                instrument_token: self.token,
                product: self.product.clone(),
                transaction_type: o.transaction_type.clone(),
                quantity: o.quantity,
                average_price: o.price,
                fill_timestamp: timestamp.clone(),
            });
            s.positions = vec![BrokerPosition {
                exchange: "MCX".into(),
                tradingsymbol: o.tradingsymbol.clone(),
                instrument_token: self.token,
                product: self.product.clone(),
                quantity: qty,
                average_price: if qty == 0 { Decimal::ZERO } else { o.price },
            }];
            let row = s
                .orders
                .iter_mut()
                .find(|b| b.order_id == o.order_id)
                .expect("mock order");
            row.status = "COMPLETE".into();
            row.filled_quantity = row.quantity;
            row.exchange_update_timestamp = Some(timestamp);
        }
        Ok(s.clone())
    }
    async fn execute(&self, command: &Command) -> Result<Outcome> {
        command.validate()?;
        if let Command::ProtectedMarket {
            symbol,
            side,
            product,
            quantity,
            tag,
            ..
        } = command
        {
            let translated = Command::Place {
                symbol: symbol.clone(),
                side: side.clone(),
                product: product.clone(),
                quantity: *quantity,
                tag: tag.clone(),
                price_rupees: 6000,
            };
            let outcome = self.execute(&translated).await?;
            if let Outcome::Acknowledged { order_id } = &outcome {
                let mut s = self.state.lock().map_err(|_| anyhow!("Mock state"))?;
                let o = s
                    .orders
                    .iter_mut()
                    .find(|o| &o.order_id == order_id)
                    .expect("mock acknowledgement");
                o.market_protection = Some(Decimal::ZERO); // observed Kite converted-order response
            }
            return Ok(outcome);
        }
        let mut s = self
            .state
            .lock()
            .map_err(|_| anyhow!("Mock broker state unavailable"))?;
        match command {
            Command::ProtectedMarket { .. } => unreachable!("handled above"),
            Command::Place {
                side,
                product,
                quantity,
                price_rupees,
                tag,
                ..
            } => {
                ensure!(product == &self.product, "Mock product mismatch");
                let id = (s.orders.len() + 1).to_string();
                let timestamp = Self::time();
                s.orders.push(BrokerOrder {
                    order_id: id.clone(),
                    exchange: "MCX".into(),
                    tradingsymbol: "CRUDEOIL26SEPFUT".into(),
                    instrument_token: self.token,
                    product: product.clone(),
                    transaction_type: side.clone(),
                    variety: "regular".into(),
                    order_type: "LIMIT".into(),
                    market_protection: None,
                    validity: "DAY".into(),
                    status: "OPEN".into(),
                    quantity: *quantity,
                    filled_quantity: 0,
                    price: Decimal::from(*price_rupees),
                    tag: Some(tag.clone()),
                    exchange_timestamp: Some(timestamp.clone()),
                    exchange_update_timestamp: Some(timestamp.clone()),
                    order_timestamp: timestamp,
                });
                Ok(Outcome::Acknowledged { order_id: id })
            }
            Command::Cancel { order_id } => {
                let o = s
                    .orders
                    .iter_mut()
                    .find(|o| &o.order_id == order_id)
                    .ok_or_else(|| anyhow!("Unknown mock broker order"))?;
                if o.status != "OPEN" {
                    return Ok(Outcome::Rejected);
                }
                o.status = "CANCELLED".into();
                o.exchange_update_timestamp = Some(Self::time());
                Ok(Outcome::Acknowledged {
                    order_id: order_id.clone(),
                })
            }
            Command::Modify { .. } => anyhow::bail!("Native Kite mock modification unsupported"),
        }
    }
}

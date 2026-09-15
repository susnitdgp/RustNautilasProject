use super::super::{
    broker_events::{BrokerOrder, BrokerTrade},
    request::Command,
    transport::{KiteOrderTransport, Outcome},
};
use crate::{
    credentials::KiteCredentials,
    http::authenticated::{Endpoint, ReadClient},
};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::Deserialize;
#[derive(Clone, Debug, Deserialize)]
pub struct BrokerPosition {
    pub exchange: String,
    pub tradingsymbol: String,
    pub instrument_token: u32,
    pub product: String,
    pub quantity: i64,
    pub average_price: Decimal,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Funds {
    pub enabled: bool,
    pub net: Decimal,
    pub utilised: Utilised,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Utilised {
    pub debits: Decimal,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub orders: Vec<BrokerOrder>,
    pub trades: Vec<BrokerTrade>,
    pub positions: Vec<BrokerPosition>,
    pub funds: Funds,
}
#[async_trait]
pub(crate) trait Broker: Send + Sync {
    async fn verify(&self) -> Result<()>;
    async fn execute(&self, _: &Command) -> Result<Outcome> {
        anyhow::bail!("Broker mutations disabled")
    }
    async fn snapshot(&self) -> Result<Snapshot>;
    async fn fees(
        &self,
        snapshot: &Snapshot,
        product: &str,
        token: u32,
    ) -> Result<super::fees::Fees> {
        ensure!(
            super::fees::groups(snapshot, product, token)?.is_empty(),
            "Kite fill commissions unavailable"
        );
        Ok(super::fees::Fees::new())
    }
}
pub(crate) struct KiteBroker {
    read: ReadClient,
    orders: KiteOrderTransport,
    user_id: String,
    product: String,
}
impl KiteBroker {
    pub fn new(credentials: &KiteCredentials, user_id: String, product: String) -> Result<Self> {
        Ok(Self {
            read: ReadClient::new(credentials)?,
            orders: KiteOrderTransport::new(credentials)?,
            user_id,
            product,
        })
    }
}
#[async_trait]
impl Broker for KiteBroker {
    async fn fees(
        &self,
        snapshot: &Snapshot,
        product: &str,
        token: u32,
    ) -> Result<super::fees::Fees> {
        super::fees::calculate(&self.read, snapshot, product, token).await
    }

    async fn execute(&self, command: &Command) -> Result<Outcome> {
        self.orders.execute(command).await
    }

    async fn verify(&self) -> Result<()> {
        #[derive(Deserialize)]
        struct Profile {
            user_id: String,
            exchanges: Vec<String>,
            products: Vec<String>,
        }
        let p: Profile = self.read.get(Endpoint::Profile).await?;
        ensure!(
            p.user_id == self.user_id
                && p.exchanges.iter().any(|x| x == "MCX")
                && p.products.contains(&self.product),
            "Kite account identity or permissions mismatch"
        );
        Ok(())
    }
    async fn snapshot(&self) -> Result<Snapshot> {
        let first: Vec<BrokerOrder> = self.read.get(Endpoint::Orders).await?;
        let trades = self.read.get(Endpoint::Trades).await?;
        #[derive(Deserialize)]
        struct Positions {
            net: Vec<BrokerPosition>,
        }
        let positions: Positions = self.read.get(Endpoint::Positions).await?;
        let funds = self.read.get(Endpoint::CommodityMargins).await?;
        let orders: Vec<BrokerOrder> = self.read.get(Endpoint::Orders).await?;
        let mut first = first;
        let mut orders = orders;
        first.sort_by(|a, b| a.order_id.cmp(&b.order_id));
        orders.sort_by(|a, b| a.order_id.cmp(&b.order_id));
        ensure!(
            first == orders,
            "Kite snapshot changed while reading; retry reconciliation"
        );
        Ok(Snapshot {
            orders,
            trades,
            positions: positions.net,
            funds,
        })
    }
}

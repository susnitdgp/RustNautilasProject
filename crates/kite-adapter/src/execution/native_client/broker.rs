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
    #[serde(skip)]
    pub ledger: Option<&'static str>,
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
    /// Full account view: orders, trades, positions and margins (start-up, reports).
    async fn snapshot(&self) -> Result<Snapshot>;
    /// Reconciliation view (kite-adapter 0.7.0): the order book and the trade book only.
    /// No positions, no margins: fills come from orders and trades; positions are checked
    /// separately by the background account audit (`positions`).
    async fn book(&self) -> Result<(Vec<BrokerOrder>, Vec<BrokerTrade>)> {
        let s = self.snapshot().await?;
        Ok((s.orders, s.trades))
    }
    /// Net positions only (background account audit).
    async fn positions(&self) -> Result<Vec<BrokerPosition>> {
        Ok(self.snapshot().await?.positions)
    }
    async fn fees(
        &self,
        snapshot: &Snapshot,
        product: &str,
        token: u32,
        symbol: &str,
    ) -> Result<super::fees::Fees> {
        ensure!(
            super::fees::groups_for(snapshot, product, token, symbol)?.is_empty(),
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
    sandbox: bool,
    verified_mcx: std::sync::atomic::AtomicBool,
}
impl KiteBroker {
    pub(crate) fn sandbox(
        credentials: &KiteCredentials,
        user_id: String,
        product: String,
    ) -> Result<Self> {
        // One client: reads keep the order connection warm (HTTP/2, multiplexed).
        let client = crate::http::client::kite_client()?;
        Ok(Self {
            read: ReadClient::with_client(credentials, client.clone())?.into_sandbox(),
            orders: KiteOrderTransport::with_client(credentials, client)?,
            user_id,
            product,
            sandbox: true,
            verified_mcx: std::sync::atomic::AtomicBool::new(false),
        })
    }
    pub fn new(credentials: &KiteCredentials, user_id: String, product: String) -> Result<Self> {
        // One client: reads keep the order connection warm (HTTP/2, multiplexed).
        let client = crate::http::client::kite_client()?;
        Ok(Self {
            sandbox: false,
            verified_mcx: std::sync::atomic::AtomicBool::new(false),
            read: ReadClient::with_client(credentials, client.clone())?,
            orders: KiteOrderTransport::with_client(credentials, client)?,
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
        symbol: &str,
    ) -> Result<super::fees::Fees> {
        if self.sandbox {
            // Explicit sandbox estimate: the sandbox has no virtual contract notes.
            let mut result = super::fees::Fees::new();
            for trades in super::fees::groups_for(snapshot, product, token, symbol)?.values() {
                result.extend(super::fees::allocate(Decimal::ZERO, trades)?);
            }
            Ok(result)
        } else {
            super::fees::calculate(&self.read, snapshot, product, token, symbol).await
        }
    }

    async fn execute(&self, command: &Command) -> Result<Outcome> {
        if self.sandbox {
            self.orders.execute_sandbox(command).await
        } else {
            self.orders.execute(command).await
        }
    }

    async fn verify(&self) -> Result<()> {
        #[derive(Deserialize)]
        struct Profile {
            user_id: String,
            exchanges: Vec<String>,
            products: Vec<String>,
        }
        self.verified_mcx
            .store(false, std::sync::atomic::Ordering::Release);
        let p: Profile = self.read.get(Endpoint::Profile).await?;
        ensure!(
            p.user_id == self.user_id
                && p.exchanges.iter().any(|x| x == "MCX")
                && p.products.contains(&self.product),
            "Kite account identity or permissions mismatch"
        );
        self.verified_mcx
            .store(true, std::sync::atomic::Ordering::Release);
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
        let funds = if self.sandbox {
            let mut funds: Funds = self.read.get(Endpoint::CommodityMargins).await?;
            funds.ledger = Some("sandbox_commodity");
            funds
        } else {
            super::margins::select(
                self.read.get(Endpoint::Margins).await?,
                self.verified_mcx.load(std::sync::atomic::Ordering::Acquire),
            )?
        };
        let orders: Vec<BrokerOrder> = self.read.get(Endpoint::Orders).await?;
        let mut first = first;
        let mut orders = orders;
        first.sort_by(|a, b| a.order_id.cmp(&b.order_id));
        orders.sort_by(|a, b| a.order_id.cmp(&b.order_id));
        ensure!(first == orders, super::outage::ReadFailure::Transient);
        Ok(Snapshot {
            orders,
            trades,
            positions: positions.net,
            funds,
        })
    }
    /// Reconciliation reads (kite-adapter 0.7.0): `/orders` and `/trades` together, one
    /// round trip. They are not read under one lock at Kite, so a trade can be ahead of
    /// the book or the book ahead of the trades; `broker_events::reconcile` classifies
    /// exactly that as `ObservationLag` (never inferring a fill) and the dispatcher reads
    /// again or leaves the order for the next pass.
    async fn book(&self) -> Result<(Vec<BrokerOrder>, Vec<BrokerTrade>)> {
        let (orders, trades) = tokio::join!(
            self.read.get::<Vec<BrokerOrder>>(Endpoint::Orders),
            self.read.get::<Vec<BrokerTrade>>(Endpoint::Trades),
        );
        Ok((orders?, trades?))
    }
    async fn positions(&self) -> Result<Vec<BrokerPosition>> {
        #[derive(Deserialize)]
        struct Positions {
            net: Vec<BrokerPosition>,
        }
        Ok(self.read.get::<Positions>(Endpoint::Positions).await?.net)
    }
}

#[cfg(test)]
#[path = "book_tests.rs"]
mod book_tests;

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
    async fn snapshot(&self) -> Result<Snapshot>;
    /// The order-path view: orders, trades and positions, as consistent as `snapshot`.
    /// Margins are not part of any order check, so a broker may return the last known
    /// funds instead of reading them again (`KiteBroker` does, kite-adapter 0.6.0).
    async fn trading_snapshot(&self) -> Result<Snapshot> {
        self.snapshot().await
    }
    /// Postback fast path: one order's latest state and its trades. `Ok(None)` means the
    /// broker does not support it (mock, tests); the full snapshot then handles the fill.
    async fn order_detail(
        &self,
        _order_id: &str,
    ) -> Result<Option<(BrokerOrder, Vec<BrokerTrade>)>> {
        Ok(None)
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
    /// Funds from the last full snapshot, reused by `trading_snapshot`.
    last_funds: std::sync::Mutex<Option<Funds>>,
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
            last_funds: std::sync::Mutex::new(None),
        })
    }
    pub fn new(credentials: &KiteCredentials, user_id: String, product: String) -> Result<Self> {
        // One client: reads keep the order connection warm (HTTP/2, multiplexed).
        let client = crate::http::client::kite_client()?;
        Ok(Self {
            sandbox: false,
            verified_mcx: std::sync::atomic::AtomicBool::new(false),
            last_funds: std::sync::Mutex::new(None),
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
    /// The order comes from the day book (`GET /orders`), the same source as the full
    /// snapshot: `GET /orders/{id}` history entries are documented without
    /// `market_protection` and `exchange_update_timestamp`, which `reconcile` needs for
    /// owned MARKET orders and for the trade/order chronology. Both reads go out
    /// together: one round trip.
    async fn order_detail(
        &self,
        order_id: &str,
    ) -> Result<Option<(BrokerOrder, Vec<BrokerTrade>)>> {
        let (book, trades) = tokio::join!(
            self.read.get::<Vec<BrokerOrder>>(Endpoint::Orders),
            self.read.order_trades::<Vec<BrokerTrade>>(order_id),
        );
        let mut matching = book?.into_iter().filter(|o| o.order_id == order_id);
        let order = matching
            .next()
            .ok_or_else(|| anyhow::anyhow!("Order not in the Kite day book yet"))?;
        ensure!(matching.next().is_none(), "Kite day book lists the order twice");
        let trades = trades?;
        ensure!(
            trades.iter().all(|t| t.order_id == order_id),
            "Kite order trades identity mismatch"
        );
        Ok(Some((order, trades)))
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
        if let Ok(mut last) = self.last_funds.lock() {
            *last = Some(funds.clone());
        }
        Ok(Snapshot {
            orders,
            trades,
            positions: positions.net,
            funds,
        })
    }
    /// Order-path snapshot (kite-adapter 0.6.0): `/orders`, `/trades` and `/positions`
    /// read together (one round trip instead of three), then `/orders` again; margins
    /// are not read (no order check uses them; the last full snapshot's funds are
    /// returned). 4 reads, ~2 round trips, instead of 5 reads one after another.
    ///
    /// Consistency is unchanged in kind: the book must be identical before and after the
    /// trades and positions were read, or the read is retried (`Transient`). If an order
    /// changes between the concurrent reads, `/trades` or `/positions` can trail the book;
    /// reconciliation already classifies exactly that as `ObservationLag` (trade sum vs
    /// filled quantity, owned fills vs position) and reads again, never inferring a fill.
    async fn trading_snapshot(&self) -> Result<Snapshot> {
        let funds = self.last_funds.lock().ok().and_then(|f| f.clone());
        let Some(funds) = funds else {
            return self.snapshot().await;
        };
        #[derive(Deserialize)]
        struct Positions {
            net: Vec<BrokerPosition>,
        }
        let (first, trades, positions) = tokio::join!(
            self.read.get::<Vec<BrokerOrder>>(Endpoint::Orders),
            self.read.get::<Vec<BrokerTrade>>(Endpoint::Trades),
            self.read.get::<Positions>(Endpoint::Positions),
        );
        let (mut first, trades, positions) = (first?, trades?, positions?);
        let mut orders: Vec<BrokerOrder> = self.read.get(Endpoint::Orders).await?;
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
}

#[cfg(test)]
#[path = "trading_snapshot_tests.rs"]
mod trading_snapshot_tests;

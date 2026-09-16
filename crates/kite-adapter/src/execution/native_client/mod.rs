pub mod coordination;
pub mod custom_sandbox;
mod dispatch;
mod fees;
mod ledger;
pub mod mock;
pub(crate) mod outage;
pub mod production;
pub mod recovery;
pub mod sandbox;
mod shutdown;
// Native Kite client. Real mutations remain disabled at this boundary.
mod broker;
pub mod margins;
mod reports;

use self::broker::{Broker, KiteBroker, Snapshot};
use anyhow::{Result, anyhow, bail, ensure};
use async_trait::async_trait;
use nautilus_common::{
    cache::CacheView,
    clients::ExecutionClient,
    factories::{ClientConfig, ExecutionClientFactory, OrderEventFactory},
    live::runner::try_get_exec_event_sender,
    messages::{ExecutionEvent, execution::*},
};
use nautilus_core::{Params, UnixNanos};
use nautilus_model::orders::Order;
use nautilus_model::{
    accounts::{AccountAny, MarginAccount},
    enums::*,
    events::OrderEventAny,
    identifiers::*,
    orders::OrderAny,
    reports::*,
    types::*,
};
use std::{any::Any, cell::RefCell, sync::Arc};
#[derive(Debug, Clone)]
pub struct Config {
    pub user_id: String,
    pub product: String,
    pub instrument_token: u32,
    pub credentials: Arc<crate::credentials::KiteCredentials>,
}
impl ClientConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl Config {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.user_id.is_empty()
                && self.user_id.len() <= 32
                && self.user_id.bytes().all(|b| b.is_ascii_alphanumeric()),
            "Invalid Kite account ID"
        );
        ensure!(
            matches!(self.product.as_str(), "MIS" | "NRML") && self.instrument_token > 0,
            "Invalid native Kite product/token"
        );
        Ok(())
    }
}
#[derive(Debug, Default)]
pub struct Factory;
impl ExecutionClientFactory for Factory {
    fn name(&self) -> &str {
        "KITE"
    }
    fn config_type(&self) -> &str {
        "KiteNativeExecutionConfig"
    }
    fn create(
        &self,
        trader_id: TraderId,
        name: &str,
        config: &dyn ClientConfig,
        _cache: CacheView,
    ) -> Result<Box<dyn ExecutionClient>> {
        let config = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| anyhow!("Invalid native Kite execution config"))?
            .clone();
        config.validate()?;
        let broker = Box::new(KiteBroker::new(
            &config.credentials,
            config.user_id.clone(),
            config.product.clone(),
        )?);
        Ok(Box::new(Client::new(trader_id, name, config, broker)?))
    }
}
pub struct Client {
    broker: Arc<dyn Broker>,
    config: Config,
    factory: OrderEventFactory,
    id: ClientId,
    connected: bool,
    production: bool,
    stop_signal: Option<Arc<std::sync::atomic::AtomicBool>>,
    account: RefCell<Option<AccountAny>>,
    dispatcher: Option<Arc<tokio::sync::Mutex<dispatch::Dispatcher>>>,
    cache: Option<CacheView>,
    active: Arc<std::sync::atomic::AtomicBool>,
    tasks: RefCell<Vec<tokio::task::JoinHandle<Result<()>>>>,
    poll: Option<tokio::task::JoinHandle<Result<()>>>,
}
impl Client {
    async fn owners(&self) -> Result<std::collections::BTreeMap<String, ClientOrderId>> {
        Ok(if let Some(d) = &self.dispatcher {
            d.lock().await.ownership()
        } else {
            std::collections::BTreeMap::new()
        })
    }

    fn new(trader: TraderId, name: &str, config: Config, broker: Box<dyn Broker>) -> Result<Self> {
        config.validate()?;
        ensure!(!name.is_empty(), "Native client name missing");
        let account = AccountId::from(format!("KITE-{}", config.user_id).as_str());
        Ok(Self {
            broker: Arc::from(broker),
            config,
            factory: OrderEventFactory::new(
                trader,
                account,
                AccountType::Margin,
                Some(Currency::INR()),
            ),
            id: ClientId::from(name),
            connected: false,
            production: false,
            stop_signal: None,
            account: RefCell::new(None),
            dispatcher: None,
            cache: None,
            active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            tasks: RefCell::new(vec![]),
            poll: None,
        })
    }
    fn now() -> UnixNanos {
        nautilus_core::time::get_atomic_clock_realtime().get_time_ns()
    }
    fn emit(event: ExecutionEvent) -> Result<()> {
        try_get_exec_event_sender()
            .ok_or_else(|| anyhow!("Native execution event channel unavailable"))?
            .send(event)
            .map_err(|_| anyhow!("Native execution event channel closed"))
    }
    async fn snapshot(&self) -> Result<Snapshot> {
        ensure!(self.is_connected(), "Native Kite client disconnected");
        outage::snapshot(self.broker.as_ref()).await
    }
    fn instrument(&self, id: Option<InstrumentId>) -> Result<()> {
        ensure!(
            id.is_none_or(|x| x == InstrumentId::from("CRUDEOIL26SEPFUT.MCX")),
            "Unsupported native Kite report instrument"
        );
        Ok(())
    }
    fn orders(
        &self,
        snapshot: &Snapshot,
        now: UnixNanos,
        owners: &std::collections::BTreeMap<String, ClientOrderId>,
    ) -> Result<Vec<OrderStatusReport>> {
        let mut ids = std::collections::BTreeSet::new();
        let grouped = fees::groups(snapshot, &self.config.product, self.config.instrument_token)?;
        snapshot
            .orders
            .iter()
            .filter(|o| {
                o.exchange == "MCX"
                    && o.tradingsymbol == "CRUDEOIL26SEPFUT"
                    && o.product == self.config.product
            })
            .map(|o| {
                ensure!(
                    o.instrument_token == self.config.instrument_token
                        && ids.insert(o.order_id.clone()),
                    "Invalid or duplicate Kite order identity"
                );
                let mut report = reports::order(
                    o,
                    self.factory.account_id(),
                    owners.get(&o.order_id).copied(),
                    now,
                )?;
                if o.filled_quantity > 0 {
                    report.avg_px = Some(fees::average(
                        grouped
                            .get(&o.order_id)
                            .ok_or_else(|| anyhow!("Filled order trades missing"))?,
                    )?);
                }
                Ok(report)
            })
            .collect()
    }
    fn range(start: Option<UnixNanos>, end: Option<UnixNanos>) -> Result<()> {
        ensure!(
            start.zip(end).is_none_or(|(a, b)| a <= b),
            "Invalid report time range"
        );
        Ok(())
    }
}
#[async_trait(?Send)]
impl ExecutionClient for Client {
    fn client_id(&self) -> ClientId {
        self.id
    }
    fn account_id(&self) -> AccountId {
        self.factory.account_id()
    }
    fn venue(&self) -> Venue {
        "MCX".into()
    }
    fn oms_type(&self) -> OmsType {
        OmsType::Netting
    }
    fn is_connected(&self) -> bool {
        self.connected && self.active.load(std::sync::atomic::Ordering::Acquire)
    }
    fn get_account(&self) -> Option<AccountAny> {
        self.account.borrow().clone()
    }
    fn provides_bulk_position_coverage(&self, _: InstrumentId) -> bool {
        false
    }
    fn generate_account_state(
        &self,
        _: Vec<AccountBalance>,
        _: Vec<MarginBalance>,
        _: bool,
        _: UnixNanos,
        _: Option<Params>,
    ) -> Result<()> {
        bail!("Kite account state must come from authenticated commodity margins")
    }
    fn start(&mut self) -> Result<()> {
        Ok(())
    }
    fn stop(&mut self) -> Result<()> {
        self.connected = false;
        self.active
            .store(false, std::sync::atomic::Ordering::Release);
        Ok(())
    }
    fn reset(&mut self) -> Result<()> {
        bail!("Native Kite client cannot reset broker state")
    }
    fn dispose(&mut self) -> Result<()> {
        self.stop()
    }
    async fn connect(&mut self) -> Result<()> {
        if self.connected {
            return Ok(());
        }
        ensure!(
            try_get_exec_event_sender().is_some(),
            "Native execution event channel unavailable"
        );
        self.broker.verify().await?;
        let snapshot = outage::snapshot(self.broker.as_ref()).await?;
        reports::positions(
            &snapshot,
            self.account_id(),
            &self.config.product,
            self.config.instrument_token,
            Self::now(),
        )?;
        if self.production {
            ensure!(
                snapshot.positions.iter().all(|p| p.quantity == 0),
                "Production startup requires an account with no open positions; review existing exposure"
            );
            ensure!(
                snapshot
                    .orders
                    .iter()
                    .all(|o| matches!(o.status.as_str(), "COMPLETE" | "CANCELLED" | "REJECTED")),
                "Production startup requires no open broker orders"
            );
        }
        let state = reports::account(&snapshot, &self.factory, Self::now())?;
        Self::emit(ExecutionEvent::Account(state.clone()))?;
        *self.account.borrow_mut() = Some(AccountAny::Margin(MarginAccount::new(state, false)));
        self.connected = true;
        self.active
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(dispatcher) = &self.dispatcher {
            let dispatcher = dispatcher.clone();
            let active = self.active.clone();
            let stop_signal = self.stop_signal.clone();
            let tx = try_get_exec_event_sender()
                .ok_or_else(|| anyhow!("Native event channel unavailable"))?;
            let production = self.production;
            self.poll = Some(tokio::spawn(async move {
                while active.load(std::sync::atomic::Ordering::Acquire) {
                    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                    if !active.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    let mut service = dispatcher.lock().await;
                    if let Err(e) = service.refresh(&tx).await {
                        service.fault();
                        active.store(false, std::sync::atomic::Ordering::Release);
                        if let Some(signal) = &stop_signal {
                            signal.store(true, std::sync::atomic::Ordering::Release);
                        }
                        eprintln!(
                            "{}",
                            serde_json::json!({"event":"native_execution_fault","requires_review":true,"live_orders_enabled":production})
                        );
                        return Err(e);
                    }
                }
                Ok(())
            }));
        }
        Ok(())
    }
    async fn disconnect(&mut self) -> Result<()> {
        self.stop()?;
        let tasks = std::mem::take(&mut *self.tasks.borrow_mut());
        let mut failure = shutdown::drain(tasks, std::time::Duration::from_secs(15))
            .await
            .err();
        if let Some(poll) = self.poll.take()
            && let Err(e) = shutdown::drain(vec![poll], std::time::Duration::from_secs(15)).await
        {
            failure = Some(e);
        }
        if let Some(d) = &self.dispatcher {
            let tx = try_get_exec_event_sender()
                .ok_or_else(|| anyhow!("Native event channel unavailable during shutdown"))?;
            let mut service = d.lock().await;
            if failure.is_some() {
                service.fault();
            }
            if let Err(e) = service.finish(&tx, failure.is_some()).await {
                failure = Some(e);
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        Ok(())
    }
    fn submit_order(&self, cmd: SubmitOrder) -> Result<()> {
        ensure!(self.is_connected(), "Native Kite client disconnected");
        ensure!(
            cmd.trader_id == self.factory.trader_id()
                && cmd.order_init.trader_id == cmd.trader_id
                && cmd.order_init.client_order_id == cmd.client_order_id
                && cmd.order_init.strategy_id == cmd.strategy_id
                && cmd.order_init.instrument_id == cmd.instrument_id,
            "Native command identity mismatch"
        );
        let order = OrderAny::from_events(vec![OrderEventAny::Initialized(cmd.order_init)])?;
        if self.production {
            ensure!(
                order.order_type() == OrderType::Market,
                "Selected production strategy submits protected market orders only"
            );
        }
        if let Some(dispatcher) = &self.dispatcher {
            let cache = self
                .cache
                .as_ref()
                .ok_or_else(|| anyhow!("Native cache unavailable"))?
                .borrow();
            let position: f64 = cache
                .positions_open(None, None, None, None, None)
                .iter()
                .filter(|p| p.instrument_id == cmd.instrument_id)
                .map(|p| p.signed_qty)
                .sum();
            ensure!(
                position.is_finite() && position.fract() == 0.0 && position.abs() <= 100.0,
                "Unsupported native position"
            );
            let dispatcher = dispatcher.clone();
            let active = self.active.clone();
            let stop_signal = self.stop_signal.clone();
            let tx = try_get_exec_event_sender()
                .ok_or_else(|| anyhow!("Native event channel unavailable"))?;
            tokio::runtime::Handle::try_current()
                .map_err(|_| anyhow!("Native runtime unavailable"))?;
            ensure!(
                self.tasks.borrow().len() < 256,
                "Native task admission capacity exhausted"
            );
            self.tasks.borrow_mut().push(tokio::spawn(async move {
                ensure!(
                    active.load(std::sync::atomic::Ordering::Acquire),
                    "Native client stopped"
                );
                let mut service = dispatcher.lock().await;
                ensure!(
                    active.load(std::sync::atomic::Ordering::Acquire),
                    "Native client stopped before dispatch"
                );
                let result = async {
                    service.submit(order, position as i64, &tx).await?;
                    service.refresh(&tx).await
                }
                .await;
                if result.is_err() {
                    service.fault();
                    active.store(false, std::sync::atomic::Ordering::Release);
                    if let Some(signal) = &stop_signal {
                        signal.store(true, std::sync::atomic::Ordering::Release);
                    }
                }
                result
            }));
            Ok(())
        } else {
            Self::emit(ExecutionEvent::Order(self.factory.generate_order_denied(
                &order,
                "Real Kite orders are disabled",
                Self::now(),
            )))
        }
    }
    fn submit_order_list(&self, _: SubmitOrderList) -> Result<()> {
        bail!("Real Kite orders are disabled")
    }
    fn modify_order(&self, _: ModifyOrder) -> Result<()> {
        bail!("Real Kite orders are disabled")
    }
    fn batch_modify_orders(&self, _: BatchModifyOrders) -> Result<()> {
        bail!("Real Kite orders are disabled")
    }
    fn cancel_order(&self, cmd: CancelOrder) -> Result<()> {
        ensure!(self.is_connected(), "Native Kite client disconnected");
        let dispatcher = self
            .dispatcher
            .as_ref()
            .ok_or_else(|| anyhow!("Real Kite order cancellation is disabled"))?
            .clone();
        ensure!(
            cmd.instrument_id == InstrumentId::from("CRUDEOIL26SEPFUT.MCX")
                && cmd.trader_id == self.factory.trader_id(),
            "Native cancel identity mismatch"
        );
        let tx = try_get_exec_event_sender()
            .ok_or_else(|| anyhow!("Native event channel unavailable"))?;
        let active = self.active.clone();
        let stop_signal = self.stop_signal.clone();
        ensure!(
            self.tasks.borrow().len() < 256,
            "Native task admission capacity exhausted"
        );
        tokio::runtime::Handle::try_current().map_err(|_| anyhow!("Native runtime unavailable"))?;
        self.tasks.borrow_mut().push(tokio::spawn(async move {
            ensure!(
                active.load(std::sync::atomic::Ordering::Acquire),
                "Native client stopped"
            );
            let mut service = dispatcher.lock().await;
            ensure!(
                active.load(std::sync::atomic::Ordering::Acquire),
                "Native client stopped before dispatch"
            );
            let result = async {
                service
                    .cancel(cmd.client_order_id, cmd.command_id, &tx)
                    .await?;
                service.refresh(&tx).await
            }
            .await;
            if result.is_err() {
                service.fault();
                active.store(false, std::sync::atomic::Ordering::Release);
                if let Some(signal) = &stop_signal {
                    signal.store(true, std::sync::atomic::Ordering::Release);
                }
            }
            result
        }));
        Ok(())
    }
    fn cancel_all_orders(&self, cmd: CancelAllOrders) -> Result<()> {
        ensure!(self.is_connected(), "Native Kite client disconnected");
        ensure!(
            self.dispatcher.is_some(),
            "Real Kite order cancellation is disabled"
        );
        ensure!(
            cmd.trader_id == self.factory.trader_id(),
            "Native cancellation trader mismatch"
        );
        let cache = self
            .cache
            .as_ref()
            .ok_or_else(|| anyhow!("Native cache unavailable"))?
            .borrow();
        let orders = cache.orders_open(
            None,
            Some(&cmd.instrument_id),
            Some(&cmd.strategy_id),
            Some(&self.account_id()),
            cmd.order_side,
        );
        for o in orders {
            self.cancel_order(CancelOrder::new(
                cmd.trader_id,
                cmd.client_id,
                cmd.strategy_id,
                cmd.instrument_id,
                o.client_order_id(),
                o.venue_order_id(),
                nautilus_core::UUID4::new(),
                Self::now(),
                None,
                None,
            ))?;
        }
        Ok(())
    }
    fn batch_cancel_orders(&self, _: BatchCancelOrders) -> Result<()> {
        bail!("Real Kite order cancellation is disabled")
    }
    fn query_account(&self, _: QueryAccount) -> Result<()> {
        bail!("Use asynchronous native reconciliation to refresh Kite account")
    }
    fn query_order(&self, _: QueryOrder) -> Result<()> {
        bail!("Use generate_order_status_report for Kite order query")
    }
    async fn generate_order_status_report(
        &self,
        cmd: &GenerateOrderStatusReport,
    ) -> Result<Option<OrderStatusReport>> {
        self.instrument(cmd.instrument_id)?;
        let owners = self.owners().await?;
        let id = if let Some(client) = cmd.client_order_id {
            let owned = owners
                .iter()
                .find(|(_, c)| **c == client)
                .map(|(id, _)| VenueOrderId::from(id.as_str()))
                .ok_or_else(|| anyhow!("Native order ownership unresolved"))?;
            ensure!(
                cmd.venue_order_id.is_none_or(|id| id == owned),
                "Native report identity mismatch"
            );
            owned
        } else {
            cmd.venue_order_id
                .ok_or_else(|| anyhow!("Kite broker order ID required"))?
        };
        let snapshot = self.snapshot().await?;
        Ok(self
            .orders(&snapshot, Self::now(), &owners)?
            .into_iter()
            .find(|o| o.venue_order_id == id))
    }
    async fn generate_order_status_reports(
        &self,
        cmd: &GenerateOrderStatusReports,
    ) -> Result<Vec<OrderStatusReport>> {
        self.instrument(cmd.instrument_id)?;
        Self::range(cmd.start, cmd.end)?;
        let snapshot = self.snapshot().await?;
        Ok(self
            .orders(&snapshot, Self::now(), &self.owners().await?)?
            .into_iter()
            .filter(|o| {
                (!cmd.open_only
                    || !matches!(
                        o.order_status,
                        OrderStatus::Filled
                            | OrderStatus::Canceled
                            | OrderStatus::Rejected
                            | OrderStatus::Expired
                    ))
                    && cmd.start.is_none_or(|s| o.ts_last >= s)
                    && cmd.end.is_none_or(|e| o.ts_last <= e)
            })
            .collect())
    }
    async fn generate_fill_reports(&self, cmd: GenerateFillReports) -> Result<Vec<FillReport>> {
        self.instrument(cmd.instrument_id)?;
        Self::range(cmd.start, cmd.end)?;
        let snapshot = self.snapshot().await?;
        let fees = self
            .broker
            .fees(
                &snapshot,
                &self.config.product,
                self.config.instrument_token,
            )
            .await?;
        let owners = self.owners().await?;
        Ok(reports::fills(
            &snapshot,
            self.account_id(),
            &self.config.product,
            self.config.instrument_token,
            Self::now(),
            &fees,
            &owners,
        )?
        .into_iter()
        .filter(|f| {
            cmd.venue_order_id.is_none_or(|id| id == f.venue_order_id)
                && cmd.start.is_none_or(|s| f.ts_event >= s)
                && cmd.end.is_none_or(|e| f.ts_event <= e)
        })
        .collect())
    }
    async fn generate_position_status_reports(
        &self,
        cmd: &GeneratePositionStatusReports,
    ) -> Result<Vec<PositionStatusReport>> {
        self.instrument(cmd.instrument_id)?;
        ensure!(
            cmd.start.is_none() && cmd.end.is_none(),
            "Kite reports current positions only"
        );
        let snapshot = self.snapshot().await?;
        reports::positions(
            &snapshot,
            self.account_id(),
            &self.config.product,
            self.config.instrument_token,
            Self::now(),
        )
    }
    async fn generate_mass_status(
        &self,
        lookback_mins: Option<u64>,
    ) -> Result<Option<ExecutionMassStatus>> {
        let snapshot = self.snapshot().await?;
        let now = Self::now();
        let start = lookback_mins
            .map(|m| {
                m.checked_mul(60_000_000_000)
                    .map(|v| UnixNanos::from(now.as_u64().saturating_sub(v)))
                    .ok_or_else(|| anyhow!("Native report lookback overflow"))
            })
            .transpose()?;
        let owners = self.owners().await?;
        let orders = self.orders(&snapshot, now, &owners)?;
        let positions = reports::positions(
            &snapshot,
            self.account_id(),
            &self.config.product,
            self.config.instrument_token,
            now,
        )?;
        let fees = self
            .broker
            .fees(
                &snapshot,
                &self.config.product,
                self.config.instrument_token,
            )
            .await?;
        let fills = reports::fills(
            &snapshot,
            self.account_id(),
            &self.config.product,
            self.config.instrument_token,
            now,
            &fees,
            &owners,
        )?;
        let state = reports::account(&snapshot, &self.factory, now)?;
        Self::emit(ExecutionEvent::Account(state.clone()))?;
        *self.account.borrow_mut() = Some(AccountAny::Margin(MarginAccount::new(state, false)));
        let mut report =
            ExecutionMassStatus::new(self.client_id(), self.account_id(), self.venue(), now, None);
        report.add_order_reports(
            orders
                .into_iter()
                .filter(|o| start.is_none_or(|s| o.ts_last >= s))
                .collect(),
        );
        report.add_fill_reports(
            fills
                .into_iter()
                .filter(|f| start.is_none_or(|s| f.ts_event >= s))
                .collect(),
        );
        report.add_position_reports(positions);
        Ok(Some(report))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod dispatch_tests;

#[cfg(test)]
mod fee_tests;

#[cfg(test)]
mod coordination_tests;

#[cfg(test)]
mod outage_tests;

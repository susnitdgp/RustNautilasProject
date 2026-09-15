//! Native Redis cache compatibility adapter for repeated matching-engine event notifications.
//! Delegates storage and reconstruction to Nautilus; suppresses exact duplicate events per fresh run.
use std::fmt::Debug;

use ahash::AHashMap;
use bytes::Bytes;
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{
    accounts::AccountAny,
    data::{
        Bar, CustomData, DataType, FundingRateUpdate, QuoteTick, TradeTick,
        greeks::{GreeksData, YieldCurveData},
    },
    events::{OrderEventAny, OrderSnapshot, position::snapshot::PositionSnapshot},
    identifiers::{
        AccountId, ActorId, ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId,
        TraderId, VenueOrderId,
    },
    instruments::{InstrumentAny, SyntheticInstrument},
    orderbook::OrderBook,
    orders::OrderAny,
    position::Position,
    types::{Currency, Money},
};
use ustr::Ustr;

use nautilus_common::cache::{
    CacheConfig,
    database::{CacheDatabaseAdapter, CacheDatabaseFactory, CacheMap},
};
use nautilus_common::signal::Signal;

use nautilus_infrastructure::redis::cache::{
    RedisCacheConfig, RedisCacheDatabase, RedisCacheDatabaseAdapter,
};
use std::{collections::HashSet, sync::Mutex};
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
        Ok(Box::new(Adapter {
            inner: RedisCacheDatabaseAdapter { database },
            seen: Mutex::new(HashSet::new()),
        }))
    }
}
struct Adapter {
    inner: RedisCacheDatabaseAdapter,
    seen: Mutex<HashSet<String>>,
}
#[async_trait::async_trait]
impl CacheDatabaseAdapter for Adapter {
    fn close(&mut self) -> anyhow::Result<()> {
        self.inner.close()
    }
    fn flush(&mut self) -> anyhow::Result<()> {
        self.inner.flush()
    }
    async fn load_all(&self) -> anyhow::Result<CacheMap> {
        self.inner.load_all().await
    }
    fn load(&self) -> anyhow::Result<AHashMap<String, Bytes>> {
        self.inner.load()
    }
    async fn load_currencies(&self) -> anyhow::Result<AHashMap<Ustr, Currency>> {
        self.inner.load_currencies().await
    }
    async fn load_instruments(&self) -> anyhow::Result<AHashMap<InstrumentId, InstrumentAny>> {
        self.inner.load_instruments().await
    }
    async fn load_synthetics(&self) -> anyhow::Result<AHashMap<InstrumentId, SyntheticInstrument>> {
        self.inner.load_synthetics().await
    }
    async fn load_accounts(&self) -> anyhow::Result<AHashMap<AccountId, AccountAny>> {
        self.inner.load_accounts().await
    }
    async fn load_orders(&self) -> anyhow::Result<AHashMap<ClientOrderId, OrderAny>> {
        self.inner.load_orders().await
    }
    async fn load_positions(&self) -> anyhow::Result<AHashMap<PositionId, Position>> {
        self.inner.load_positions().await
    }
    async fn load_greeks(&self) -> anyhow::Result<AHashMap<InstrumentId, GreeksData>> {
        self.inner.load_greeks().await
    }
    async fn load_yield_curves(&self) -> anyhow::Result<AHashMap<String, YieldCurveData>> {
        self.inner.load_yield_curves().await
    }
    fn load_index_order_position(&self) -> anyhow::Result<AHashMap<ClientOrderId, PositionId>> {
        self.inner.load_index_order_position()
    }
    fn load_index_order_client(&self) -> anyhow::Result<AHashMap<ClientOrderId, ClientId>> {
        self.inner.load_index_order_client()
    }
    async fn load_currency(&self, code: &Ustr) -> anyhow::Result<Option<Currency>> {
        self.inner.load_currency(code).await
    }
    async fn load_instrument(
        &self,
        instrument_id: &InstrumentId,
    ) -> anyhow::Result<Option<InstrumentAny>> {
        self.inner.load_instrument(instrument_id).await
    }
    async fn load_synthetic(
        &self,
        instrument_id: &InstrumentId,
    ) -> anyhow::Result<Option<SyntheticInstrument>> {
        self.inner.load_synthetic(instrument_id).await
    }
    async fn load_account(&self, account_id: &AccountId) -> anyhow::Result<Option<AccountAny>> {
        self.inner.load_account(account_id).await
    }
    async fn load_order(
        &self,
        client_order_id: &ClientOrderId,
    ) -> anyhow::Result<Option<OrderAny>> {
        self.inner.load_order(client_order_id).await
    }
    async fn load_position(&self, position_id: &PositionId) -> anyhow::Result<Option<Position>> {
        self.inner.load_position(position_id).await
    }
    fn load_actor(&self, actor_id: &ActorId) -> anyhow::Result<AHashMap<String, Bytes>> {
        self.inner.load_actor(actor_id)
    }
    fn load_strategy(&self, strategy_id: &StrategyId) -> anyhow::Result<AHashMap<String, Bytes>> {
        self.inner.load_strategy(strategy_id)
    }
    fn load_signals(&self, name: &str) -> anyhow::Result<Vec<Signal>> {
        self.inner.load_signals(name)
    }
    fn load_custom_data(&self, data_type: &DataType) -> anyhow::Result<Vec<CustomData>> {
        self.inner.load_custom_data(data_type)
    }
    fn load_order_snapshot(
        &self,
        client_order_id: &ClientOrderId,
    ) -> anyhow::Result<Option<OrderSnapshot>> {
        self.inner.load_order_snapshot(client_order_id)
    }
    fn load_position_snapshot(
        &self,
        position_id: &PositionId,
    ) -> anyhow::Result<Option<PositionSnapshot>> {
        self.inner.load_position_snapshot(position_id)
    }
    fn load_quotes(&self, instrument_id: &InstrumentId) -> anyhow::Result<Vec<QuoteTick>> {
        self.inner.load_quotes(instrument_id)
    }
    fn load_trades(&self, instrument_id: &InstrumentId) -> anyhow::Result<Vec<TradeTick>> {
        self.inner.load_trades(instrument_id)
    }
    fn load_funding_rates(
        &self,
        instrument_id: &InstrumentId,
    ) -> anyhow::Result<Vec<FundingRateUpdate>> {
        self.inner.load_funding_rates(instrument_id)
    }
    fn load_bars(&self, instrument_id: &InstrumentId) -> anyhow::Result<Vec<Bar>> {
        self.inner.load_bars(instrument_id)
    }
    fn add(&self, key: String, value: Bytes) -> anyhow::Result<()> {
        self.inner.add(key, value)
    }
    fn add_currency(&self, currency: &Currency) -> anyhow::Result<()> {
        self.inner.add_currency(currency)
    }
    fn add_instrument(&self, instrument: &InstrumentAny) -> anyhow::Result<()> {
        self.inner.add_instrument(instrument)
    }
    fn add_synthetic(&self, synthetic: &SyntheticInstrument) -> anyhow::Result<()> {
        self.inner.add_synthetic(synthetic)
    }
    fn add_account(&self, account: &AccountAny) -> anyhow::Result<()> {
        self.inner.add_account(account)
    }
    fn add_order(&self, order: &OrderAny, client_id: Option<ClientId>) -> anyhow::Result<()> {
        self.inner.add_order(order, client_id)
    }
    fn add_order_snapshot(&self, snapshot: &OrderSnapshot) -> anyhow::Result<()> {
        self.inner.add_order_snapshot(snapshot)
    }
    fn add_position(&self, position: &Position) -> anyhow::Result<()> {
        self.inner.add_position(position)
    }
    fn add_position_snapshot(&self, snapshot: &PositionSnapshot) -> anyhow::Result<()> {
        self.inner.add_position_snapshot(snapshot)
    }
    fn add_order_book(&self, order_book: &OrderBook) -> anyhow::Result<()> {
        self.inner.add_order_book(order_book)
    }
    fn add_signal(&self, signal: &Signal) -> anyhow::Result<()> {
        self.inner.add_signal(signal)
    }
    fn add_custom_data(&self, data: &CustomData) -> anyhow::Result<()> {
        self.inner.add_custom_data(data)
    }
    fn add_quote(&self, quote: &QuoteTick) -> anyhow::Result<()> {
        self.inner.add_quote(quote)
    }
    fn add_trade(&self, trade: &TradeTick) -> anyhow::Result<()> {
        self.inner.add_trade(trade)
    }
    fn add_funding_rate(&self, funding_rate: &FundingRateUpdate) -> anyhow::Result<()> {
        self.inner.add_funding_rate(funding_rate)
    }
    fn add_bar(&self, bar: &Bar) -> anyhow::Result<()> {
        self.inner.add_bar(bar)
    }
    fn add_greeks(&self, _greeks: &GreeksData) -> anyhow::Result<()> {
        self.inner.add_greeks(_greeks)
    }
    fn add_yield_curve(&self, _yield_curve: &YieldCurveData) -> anyhow::Result<()> {
        self.inner.add_yield_curve(_yield_curve)
    }
    fn delete_actor(&self, actor_id: &ActorId) -> anyhow::Result<()> {
        self.inner.delete_actor(actor_id)
    }
    fn delete_strategy(&self, component_id: &StrategyId) -> anyhow::Result<()> {
        self.inner.delete_strategy(component_id)
    }
    fn delete_order(&self, client_order_id: &ClientOrderId) -> anyhow::Result<()> {
        self.inner.delete_order(client_order_id)
    }
    fn delete_position(&self, position_id: &PositionId) -> anyhow::Result<()> {
        self.inner.delete_position(position_id)
    }
    fn delete_account_event(&self, account_id: &AccountId, event_id: &str) -> anyhow::Result<()> {
        self.inner.delete_account_event(account_id, event_id)
    }
    fn index_venue_order_id(
        &self,
        client_order_id: ClientOrderId,
        venue_order_id: VenueOrderId,
    ) -> anyhow::Result<()> {
        self.inner
            .index_venue_order_id(client_order_id, venue_order_id)
    }
    fn index_order_position(
        &self,
        client_order_id: ClientOrderId,
        position_id: PositionId,
    ) -> anyhow::Result<()> {
        self.inner
            .index_order_position(client_order_id, position_id)
    }
    fn index_order_clients(&self, claims: &[(ClientOrderId, ClientId)]) -> anyhow::Result<()> {
        self.inner.index_order_clients(claims)
    }
    fn update_actor(
        &self,
        actor_id: &ActorId,
        state: &AHashMap<String, Bytes>,
    ) -> anyhow::Result<()> {
        self.inner.update_actor(actor_id, state)
    }
    fn update_strategy(
        &self,
        strategy_id: &StrategyId,
        state: &AHashMap<String, Bytes>,
    ) -> anyhow::Result<()> {
        self.inner.update_strategy(strategy_id, state)
    }
    fn update_account(&self, account: &AccountAny) -> anyhow::Result<()> {
        self.inner.update_account(account)
    }
    fn update_order(&self, order_event: &OrderEventAny) -> anyhow::Result<()> {
        let key = serde_json::to_string(order_event)?;
        let mut seen = self
            .seen
            .lock()
            .map_err(|_| anyhow::anyhow!("Redis event identity lock poisoned"))?;
        if seen.contains(&key) {
            return Ok(());
        }
        self.inner.update_order(order_event)?;
        seen.insert(key);
        Ok(())
    }
    fn update_position(&self, position: &Position) -> anyhow::Result<()> {
        self.inner.update_position(position)
    }
    fn snapshot_order_state(&self, order: &OrderAny) -> anyhow::Result<()> {
        self.inner.snapshot_order_state(order)
    }
    fn snapshot_position_state(
        &self,
        position: &Position,
        ts_snapshot: UnixNanos,
        unrealized_pnl: Option<Money>,
    ) -> anyhow::Result<()> {
        self.inner
            .snapshot_position_state(position, ts_snapshot, unrealized_pnl)
    }
    fn heartbeat(&self, timestamp: UnixNanos) -> anyhow::Result<()> {
        self.inner.heartbeat(timestamp)
    }
}

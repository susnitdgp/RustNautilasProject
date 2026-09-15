//! Isolated native emulator/TWAP integration scenarios; synthetic data only.
use anyhow::{Result, ensure};
use nautilus_backtest::{
    config::{BacktestEngineConfig, SimulatedVenueConfig},
    engine::BacktestEngine,
};
use nautilus_common::{
    actor::DataActor, cache::database::CacheDatabaseFactory, logging::logger::LoggerConfig,
};
use nautilus_core::UUID4;
use nautilus_model::{
    data::{Data, QuoteTick},
    enums::{AccountType, BookType, OmsType, OrderSide, TimeInForce, TriggerType},
    events::{OrderEmulated, OrderFilled},
    identifiers::InstrumentId,
    instruments::InstrumentAny,
    orders::Order,
    types::{Money, Price, Quantity},
};
use nautilus_trading::{
    algorithm::{ExecutionAlgorithmConfig, twap::TwapAlgorithm},
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
use std::{cell::Cell, rc::Rc};
#[derive(Debug)]
struct Probe {
    core: StrategyCore,
    id: InstrumentId,
    twap: bool,
    ticks: u32,
    exit: bool,
    emulated: Rc<Cell<u64>>,
    fills: Rc<Cell<u64>>,
}
impl DataActor for Probe {
    fn on_start(&mut self) -> Result<()> {
        self.subscribe_quotes(self.id, None, None);
        Ok(())
    }
    fn on_stop(&mut self) -> Result<()> {
        self.cancel_all_orders(self.id, None, None, true, None)
    }
    fn on_quote(&mut self, q: &QuoteTick) -> Result<()> {
        self.ticks += 1;
        if self.ticks == 1 {
            let order = if self.twap {
                self.order().market(
                    self.id,
                    OrderSide::Buy,
                    Quantity::from(2),
                    Some(TimeInForce::Day),
                    None,
                    None,
                    Some("TWAP".into()),
                    Some(indexmap::IndexMap::from([
                        ("horizon_secs".into(), "2".into()),
                        ("interval_secs".into(), "1".into()),
                    ])),
                    None,
                    None,
                )
            } else {
                self.order().stop_market(
                    self.id,
                    OrderSide::Buy,
                    Quantity::from(1),
                    Price::new(q.ask_price.as_f64() + 2.0, 0),
                    Some(TriggerType::BidAsk),
                    Some(TimeInForce::Day),
                    None,
                    None,
                    None,
                    None,
                    Some(TriggerType::BidAsk),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            };
            self.submit_order(order, None, None, None)?;
        }
        let target = if self.twap { 2.0 } else { 1.0 };
        let net: f64 = self
            .cache()
            .positions_open(None, Some(&self.id), None, None, None)
            .iter()
            .map(|p| p.signed_qty)
            .sum();
        if !self.exit && self.ticks >= 5 && net == target {
            self.exit = true;
            let order = self.order().market(
                self.id,
                OrderSide::Sell,
                Quantity::from(target as u64),
                Some(TimeInForce::Day),
                Some(true),
                None,
                None,
                None,
                None,
                None,
            );
            self.submit_order(order, None, None, None)?;
        }
        Ok(())
    }
}
nautilus_strategy!(Probe, {
    fn on_order_emulated(&mut self, _: OrderEmulated) {
        self.emulated.set(self.emulated.get() + 1);
    }
    fn on_order_filled(&mut self, _: &OrderFilled) {
        self.fills.set(self.fills.get() + 1);
    }
});
pub fn run(twap: bool) -> Result<()> {
    let (instrument, ts) = crate::paper_flow::simulation::fixture()?;
    let instance = UUID4::new();
    let cache_config = super::persistence::cache_config();
    let config = BacktestEngineConfig {
        trader_id: "SUSANTA-001".into(),
        instance_id: Some(instance),
        cache: Some(cache_config.clone()),
        logging: LoggerConfig {
            stdout_level: log::LevelFilter::Warn,
            is_colored: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut engine = BacktestEngine::new(config)?;
    let db = nautilus_common::live::get_runtime().block_on(
        super::redis_cache::Factory(super::persistence::redis_config()?).create(
            "SUSANTA-001".into(),
            instance,
            cache_config,
        ),
    )?;
    engine.kernel_mut().cache.borrow_mut().set_database(db);
    engine.add_venue(
        SimulatedVenueConfig::builder()
            .venue("MCX".into())
            .oms_type(OmsType::Netting)
            .account_type(AccountType::Margin)
            .book_type(BookType::L1_MBP)
            .starting_balances(vec![Money::from("1000000 INR")])
            .build()?,
    )?;
    engine.add_instrument(&InstrumentAny::FuturesContract(instrument.clone()))?;
    let emulated = Rc::new(Cell::new(0));
    let fills = Rc::new(Cell::new(0));
    engine.add_strategy(Probe {
        core: StrategyCore::new(StrategyConfig {
            strategy_id: Some("PROBE-001".into()),
            ..Default::default()
        }),
        id: instrument.id,
        twap,
        ticks: 0,
        exit: false,
        emulated: emulated.clone(),
        fills: fills.clone(),
    })?;
    if twap {
        engine.add_exec_algorithm(TwapAlgorithm::new(ExecutionAlgorithmConfig {
            exec_algorithm_id: Some("TWAP".into()),
            ..Default::default()
        }))?;
    }
    let quotes = [6000, 6001, 6003, 6004, 6005, 6006, 6006]
        .iter()
        .enumerate()
        .map(|(i, p)| {
            Data::Quote(QuoteTick::new(
                instrument.id,
                Price::new(f64::from(*p), 0),
                Price::new(f64::from(*p + 1), 0),
                Quantity::from(10),
                Quantity::from(10),
                (ts.as_u64() + (i as u64 + 1) * 1_000_000_000).into(),
                (ts.as_u64() + (i as u64 + 1) * 1_000_000_000).into(),
            ))
        })
        .collect();
    engine.add_data(quotes, None, true, true)?;
    engine.run(None, None, None, false)?;
    let cache = engine.kernel().cache.borrow();
    let net: f64 = cache
        .positions_open(None, None, None, None, None)
        .iter()
        .map(|p| p.signed_qty)
        .sum();
    let orders = cache.orders(None, None, None, None, None);
    ensure!(net == 0.0, "Component scenario did not finish flat");
    ensure!(fills.get() >= 2, "Native matching fills missing");
    if !twap {
        ensure!(
            emulated.get() > 0,
            "Native emulator did not receive the order"
        );
    }
    let children = orders
        .iter()
        .filter(|o| o.exec_spawn_id().is_some() && o.exec_spawn_id() != Some(o.client_order_id()))
        .count();
    if twap {
        ensure!(children > 0, "TWAP did not spawn child orders");
    }
    let output = serde_json::json!({"event":"native_component_verified","component":if twap {"TWAP"}else{"OrderEmulator"},
  "namespace":instance.to_string(),"fills":fills.get(),"emulated":emulated.get(),"spawned_children":children,"open_contracts":net,
  "native_matching_engine":true,"native_redis_cache":true,"live_orders_enabled":false,"broker_accessed":false});
    drop(orders);
    drop(cache);
    engine.dispose();
    println!("{}", output);
    Ok(())
}

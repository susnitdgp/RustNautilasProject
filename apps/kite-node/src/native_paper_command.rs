use crate::runtime::core::Core;
use anyhow::{Result, ensure};
use kite_adapter::{instruments::contract, preflight::Report};
use kite_paper::{
    client::PaperExecutionClient,
    events,
    outbox::Outbox,
    worker::{Handle, Request},
};
use kite_strategy::{
    checkpoint::Snapshot,
    config::Config,
    crossover::{Signal, Strategy},
};
use nautilus_common::{
    messages::execution::{CancelOrder, SubmitOrder, TradingCommand},
    msgbus,
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_execution::engine::ExecutionEngine;
use nautilus_model::{
    accounts::{AccountAny, MarginAccount},
    data::QuoteTick,
    enums::*,
    events::{AccountState, OrderEventAny},
    identifiers::*,
    orders::{LimitOrder, Order, OrderAny},
    types::*,
};

pub(crate) fn account(ts: UnixNanos) -> AccountAny {
    let currency = Currency::INR();
    AccountAny::Margin(MarginAccount::new(
        AccountState::new(
            events::account_id(),
            AccountType::Margin,
            vec![AccountBalance::new(
                Money::new(1_000_000.0, currency),
                Money::new(0.0, currency),
                Money::new(1_000_000.0, currency),
            )],
            vec![],
            false,
            UUID4::new(),
            ts,
            ts,
            Some(currency),
        ),
        false,
    ))
}
pub(crate) fn order(id: &str, side: OrderSide, price: Price, ts: UnixNanos) -> OrderAny {
    OrderAny::Limit(LimitOrder::new(
        msgbus::get_message_bus().borrow().trader_id,
        StrategyId::from("CROSSOVER-001"),
        InstrumentId::from("CRUDEOIL26SEPFUT.MCX"),
        ClientOrderId::from(id),
        side,
        Quantity::from(1),
        price,
        TimeInForce::Day,
        None,
        false,
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        UUID4::new(),
        ts,
    ))
}
fn submit(engine: &ExecutionEngine, o: &OrderAny, ts: UnixNanos) {
    engine.execute(TradingCommand::SubmitOrder(SubmitOrder::from_order(
        o,
        o.trader_id(),
        Some(events::client_id()),
        None,
        UUID4::new(),
        ts,
    )));
}
fn cancel(engine: &ExecutionEngine, id: ClientOrderId, ts: UnixNanos) {
    engine.execute(TradingCommand::CancelOrder(CancelOrder::new(
        msgbus::get_message_bus().borrow().trader_id,
        Some(events::client_id()),
        StrategyId::from("CROSSOVER-001"),
        InstrumentId::from("CRUDEOIL26SEPFUT.MCX"),
        id,
        None,
        UUID4::new(),
        ts,
        None,
        None,
    )));
}
pub(crate) fn apply(
    core: &Core,
    engine: &mut ExecutionEngine,
    batch: Vec<OrderEventAny>,
) -> Result<(usize, usize)> {
    let mut fills = 0;
    let mut cancels = 0;
    for event in batch {
        if let OrderEventAny::Initialized(init) = &event {
            if core.cache.borrow().order(&init.client_order_id).is_none() {
                core.cache.borrow_mut().add_order(
                    OrderAny::from_events(vec![event.clone()])?,
                    None,
                    Some(events::client_id()),
                    false,
                )?;
            }
            continue;
        }
        engine.process(&event);
        match &event {
            OrderEventAny::Filled(f) => {
                ensure!(
                    core.cache
                        .borrow()
                        .order(&f.client_order_id)
                        .is_some_and(|o| o.status() == OrderStatus::Filled),
                    "Native fill did not update cache"
                );
                fills += 1;
            }
            OrderEventAny::Canceled(c) => {
                ensure!(
                    core.cache
                        .borrow()
                        .order(&c.client_order_id)
                        .is_some_and(|o| o.status() == OrderStatus::Canceled),
                    "Native cancel did not update cache"
                );
                cancels += 1;
            }
            OrderEventAny::Accepted(a) => ensure!(
                core.cache
                    .borrow()
                    .order(&a.client_order_id)
                    .is_some_and(|o| o.status() == OrderStatus::Accepted),
                "Native acceptance did not update cache"
            ),
            _ => {}
        }
    }
    Ok((fills, cancels))
}
pub(crate) fn position_economics(core: &Core) -> Vec<String> {
    let cache = core.cache.borrow();
    let mut positions = cache.positions_open(None, None, None, None, None);
    positions.extend(cache.positions_closed(None, None, None, None, None));
    let mut result = positions
        .iter()
        .map(|p| {
            serde_json::json!({
                "opening_order":p.opening_order_id, "closing_order":p.closing_order_id,
                "quantity":p.quantity,"signed_qty":p.signed_qty,"multiplier":p.multiplier,
                "avg_px_open":p.avg_px_open,"avg_px_close":p.avg_px_close,
                "realized_pnl":p.realized_pnl,"buy_qty":p.buy_qty,"sell_qty":p.sell_qty
            })
            .to_string()
        })
        .collect::<Vec<_>>();
    result.sort();
    result
}
pub fn run(config_path: &str) -> Result<()> {
    let config = Config::parse(&std::fs::read_to_string(config_path)?)?;
    let namespace = UUID4::new().to_string();
    let url = kite_journal::connection::url_from_env()?;
    let result = run_at(&url, &namespace, config)?;
    println!(
        "{}",
        serde_json::json!({"namespace":namespace,"persistence":"redis_aof","result":result})
    );
    Ok(())
}
pub fn run_at(url: &str, namespace: &str, config: Config) -> Result<serde_json::Value> {
    let ts = UnixNanos::from(1_789_450_000_000_000_000_u64);
    let report = Report {
        mode: "data_only",
        instrument_id: "CRUDEOIL26SEPFUT.MCX".into(),
        instrument_token: 144870151,
        expiry: "2026-09-21".into(),
        tick_size: "1".into(),
        broker_lot_size: 1,
        validation_date_ist: chrono::NaiveDate::from_ymd_opt(2026, 9, 15).unwrap(),
        live_orders_enabled: false,
        contract_multiplier_verified: false,
        engine_started: false,
    };
    let instrument = contract::build(&report, ts)?;
    let mut core = Core::new(&instrument);
    let synthetic_account = account(ts);
    core.cache
        .borrow_mut()
        .add_account(synthetic_account.clone())?;
    let worker = Handle::start(url.to_owned(), namespace.to_owned(), config.clone())?;
    let mut engine = ExecutionEngine::new(core.clock.clone(), core.cache.clone(), None);
    engine.register_client(Box::new(PaperExecutionClient::new(
        &worker,
        synthetic_account,
    )))?;
    engine.start();
    let mut strategy = Strategy::new(config)?;
    let mut pending: Option<(ClientOrderId, Signal, u32)> = None;
    let mut signals = 0;
    let mut fills = 0;
    let mut cancellations = 0;
    let prices = [
        6008, 6006, 6004, 6002, 6000, 6002, 6004, 6006, 6005, 6005, 6003, 6003, 6001, 5999, 6000,
    ];
    for (index, p) in prices.iter().enumerate() {
        let t = UnixNanos::from(ts.as_u64() + index as u64 * 1_000_000_000);
        let quote = QuoteTick::new(
            instrument.id,
            Price::new(*p as f64, 0),
            Price::new((*p + 1) as f64, 0),
            Quantity::from(10),
            Quantity::from(10),
            t,
            t,
        );
        core.quote(quote);
        let signal = strategy.on_quote(&quote)?;
        worker.send(Request::Quote(quote))?;
        let (new_fills, _) = apply(&core, &mut engine, worker.receive()?)?;
        if new_fills > 0 {
            let (_, signal, _) = pending
                .take()
                .ok_or_else(|| anyhow::anyhow!("Unexpected native fill"))?;
            strategy.filled(signal)?;
            fills += new_fills;
        } else if let Some((id, _, age)) = &mut pending {
            *age += 1;
            if *age >= 3 || strategy.mids.is_empty() {
                cancel(&engine, *id, t);
                let (_, count) = apply(&core, &mut engine, worker.receive()?)?;
                cancellations += count;
                strategy.cancelled();
                pending = None;
            }
        }
        if let Some(signal) = signal {
            signals += 1;
            worker.send(Request::Checkpoint(Snapshot {
                version: 1,
                cursor: index,
                strategy: strategy.clone(),
                finished: false,
            }))?;
            worker.receive()?;
            let side = if matches!(signal, Signal::Buy) {
                OrderSide::Buy
            } else {
                OrderSide::Sell
            };
            let price = if side == OrderSide::Buy {
                quote.ask_price
            } else {
                quote.bid_price
            };
            let o = order(&format!("N{signals}"), side, price, t);
            let id = o.client_order_id();
            submit(&engine, &o, t);
            apply(&core, &mut engine, worker.receive()?)?;
            pending = Some((id, signal, 0));
        }
        worker.send(Request::Checkpoint(Snapshot {
            version: 1,
            cursor: index + 1,
            strategy: strategy.clone(),
            finished: false,
        }))?;
        worker.receive()?;
    }
    let end = UnixNanos::from(ts.as_u64() + 16_000_000_000);
    if let Some((id, _, _)) = pending {
        cancel(&engine, id, end);
        let (_, count) = apply(&core, &mut engine, worker.receive()?)?;
        cancellations += count;
        strategy.cancelled();
    }
    // Explicit nonmarketable order proves the native cancellation route.
    let probe = if strategy.position == 0 {
        order("CANCEL1", OrderSide::Buy, Price::new(1.0, 0), end)
    } else {
        order("CANCEL1", OrderSide::Sell, Price::new(1_000_000.0, 0), end)
    };
    submit(&engine, &probe, end);
    apply(&core, &mut engine, worker.receive()?)?;
    cancel(&engine, probe.client_order_id(), end);
    let (_, count) = apply(&core, &mut engine, worker.receive()?)?;
    cancellations += count;
    worker.send(Request::Checkpoint(Snapshot {
        version: 1,
        cursor: prices.len(),
        strategy: strategy.clone(),
        finished: true,
    }))?;
    worker.receive()?;
    engine.stop();
    drop(worker);
    let recovered = kite_journal::store::Journal::open_at(url, namespace)?;
    let net: i64 = recovered
        .state()
        .orders()
        .map(|o| {
            if matches!(o.intent.side, kite_journal::model::Side::Buy) {
                i64::from(o.filled)
            } else {
                -i64::from(o.filled)
            }
        })
        .sum();
    ensure!(
        net == i64::from(strategy.position),
        "Redis and strategy position differ"
    );
    let closed = core
        .cache
        .borrow()
        .positions_closed(None, None, None, None, None)
        .len();
    let open = core
        .cache
        .borrow()
        .positions_open(None, None, None, None, None)
        .len();
    let native_net: f64 = core
        .cache
        .borrow()
        .positions_open(None, None, None, None, None)
        .iter()
        .map(|p| p.signed_qty)
        .sum();
    ensure!(
        native_net == f64::from(strategy.position),
        "Native and strategy positions differ"
    );
    let persisted = Outbox::read_at(url, namespace)?;
    let event_count = persisted.len();
    // Fresh cache/engine, no registered client: replay cannot resubmit an order.
    let replay = Core::new(&instrument);
    replay.cache.borrow_mut().add_account(account(ts))?;
    let mut replay_engine = ExecutionEngine::new(replay.clock.clone(), replay.cache.clone(), None);
    // Match the original netting mode without registering a submission client.
    replay_engine.register_oms_type(StrategyId::from("CROSSOVER-001"), OmsType::Netting);
    apply(&replay, &mut replay_engine, persisted)?;
    let replay_open = replay
        .cache
        .borrow()
        .positions_open(None, None, None, None, None)
        .len();
    let replay_closed = replay
        .cache
        .borrow()
        .positions_closed(None, None, None, None, None)
        .len();
    ensure!(
        open == replay_open && closed == replay_closed,
        "Native replay positions differ"
    );
    ensure!(
        position_economics(&core) == position_economics(&replay),
        "Native replay position economics differ"
    );
    for o in recovered.state().orders() {
        let id = ClientOrderId::from(o.intent.id.as_str());
        ensure!(
            core.cache.borrow().order(&id).map(|x| x.status())
                == replay.cache.borrow().order(&id).map(|x| x.status()),
            "Native replay order differs"
        );
    }
    Ok(
        serde_json::json!({"event":"nautilus_paper_complete","execution_engine_started":true,"native_execution_client":true,"quotes":prices.len(),"signals":signals,"paper_fills":fills,"cancelled":cancellations,"open_contracts":net,"native_closed_positions":closed,"native_events":event_count,"native_replay_verified":true,"checkpoint_verified":true,"broker_accessed":false,"live_orders_enabled":false}),
    )
}

#[cfg(test)]
#[path = "../../../crates/kite-journal/test-support/redis.rs"]
pub(crate) mod redis_support;
#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_common::clients::ExecutionClient;
    fn config() -> Config {
        Config::parse(include_str!("../../../config/strategy-crossover.toml")).unwrap()
    }
    fn init(id: &str) -> nautilus_model::events::OrderInitialized {
        msgbus::set_message_bus(std::rc::Rc::new(std::cell::RefCell::new(
            msgbus::MessageBus::default(),
        )));
        let t = UnixNanos::from(1_789_450_000_000_000_000_u64);
        let o = order(id, OrderSide::Buy, Price::new(6000.0, 0), t);
        SubmitOrder::from_order(
            &o,
            o.trader_id(),
            Some(events::client_id()),
            None,
            UUID4::new(),
            t,
        )
        .order_init
    }
    #[test]
    fn native_engine_roundtrip_survives_redis_restart() {
        let mut redis = redis_support::TestRedis::new();
        let result = run_at(&redis.url, "native", config()).unwrap();
        assert_eq!(result["paper_fills"], 2);
        assert_eq!(result["signals"], 2);
        assert_eq!(result["cancelled"], 1);
        assert_eq!(result["native_closed_positions"], 1);
        assert_eq!(result["native_replay_verified"], true);
        assert_eq!(result["open_contracts"], 0);
        let before =
            serde_json::to_string(&Outbox::read_at(&redis.url, "native").unwrap()).unwrap();
        redis.restart();
        let after = serde_json::to_string(&Outbox::read_at(&redis.url, "native").unwrap()).unwrap();
        assert_eq!(before, after);
        assert!(Handle::start(redis.url.clone(), "native".into(), config()).is_err());
    }
    #[test]
    fn invalid_quote_cannot_fill_and_stops_worker() {
        let redis = redis_support::TestRedis::new();
        let worker = Handle::start(redis.url.clone(), "stale".into(), config()).unwrap();
        let o = init("STALE1");
        worker.send(Request::Submit(Box::new(o.clone()))).unwrap();
        worker.receive().unwrap();
        let source = UnixNanos::from(o.ts_init.as_u64() + 1);
        let q = QuoteTick::new(
            o.instrument_id,
            Price::new(5998.0, 0),
            Price::new(5999.0, 0),
            Quantity::from(10),
            Quantity::from(10),
            source,
            UnixNanos::from(source.as_u64() + 31_000_000_000),
        );
        worker.send(Request::Quote(q)).unwrap();
        assert!(worker.receive().is_err());
        drop(worker);
        let journal = kite_journal::store::Journal::open_at(&redis.url, "stale").unwrap();
        assert_eq!(journal.state().orders().next().unwrap().filled, 0);
        assert_eq!(Outbox::read_at(&redis.url, "stale").unwrap().len(), 3);
    }
    #[test]
    fn duplicate_order_cannot_be_dispatched_twice() {
        let redis = redis_support::TestRedis::new();
        let worker = Handle::start(redis.url.clone(), "duplicate".into(), config()).unwrap();
        let o = init("DUP1");
        worker.send(Request::Submit(Box::new(o.clone()))).unwrap();
        worker.receive().unwrap();
        worker.send(Request::Submit(Box::new(o))).unwrap();
        assert!(worker.receive().is_err());
        drop(worker);
        let journal = kite_journal::store::Journal::open_at(&redis.url, "duplicate").unwrap();
        assert_eq!(journal.state().orders().count(), 1);
        assert_eq!(Outbox::read_at(&redis.url, "duplicate").unwrap().len(), 3);
    }
    #[test]
    fn outbox_conflict_never_publishes_unpersisted_acceptance() {
        let redis = redis_support::TestRedis::new();
        let worker = Handle::start(redis.url.clone(), "conflict".into(), config()).unwrap();
        let _: () = redis::cmd("SET")
            .arg("susanta:nautilus:sim:native-events:{conflict}")
            .arg("corrupt")
            .query(&mut redis.connection())
            .unwrap();
        worker
            .send(Request::Submit(Box::new(init("CONFLICT1"))))
            .unwrap();
        assert!(worker.receive().is_err());
        drop(worker);
        assert!(Handle::start(redis.url.clone(), "conflict".into(), config()).is_err());
        assert_eq!(
            kite_journal::store::Journal::open_at(&redis.url, "conflict")
                .unwrap()
                .state()
                .orders()
                .count(),
            1
        );
    }
    #[test]
    fn native_client_rejects_unsupported_quantity_before_journaling() {
        let redis = redis_support::TestRedis::new();
        let worker = Handle::start(redis.url.clone(), "quantity".into(), config()).unwrap();
        let o = init("QTY1");
        let mut client = PaperExecutionClient::new(&worker, account(o.ts_init));
        client.start().unwrap();
        let native = OrderAny::from_events(vec![OrderEventAny::Initialized(o.clone())]).unwrap();
        let mut cmd = SubmitOrder::from_order(
            &native,
            native.trader_id(),
            Some(events::client_id()),
            None,
            UUID4::new(),
            o.ts_init,
        );
        cmd.order_init.quantity = Quantity::from(2);
        assert!(client.submit_order(cmd).is_err());
        assert_eq!(
            kite_journal::store::Journal::open_at(&redis.url, "quantity")
                .unwrap()
                .record_count(),
            0
        );
        client.stop().unwrap();
        client.stop().unwrap();
        assert!(!client.is_connected());
    }
}

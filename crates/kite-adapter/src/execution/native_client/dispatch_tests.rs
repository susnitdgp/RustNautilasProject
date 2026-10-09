use super::super::{request::Command, transport::Outcome};
use super::{
    broker::{Broker, Snapshot},
    dispatch::Dispatcher,
    ledger::{Record, Store},
    mock::MockBroker,
};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use nautilus_common::{
    clock::TestClock,
    factories::{OrderEventFactory, OrderFactory},
    messages::ExecutionEvent,
};
use nautilus_core::UUID4;
use nautilus_model::{
    enums::*,
    events::OrderEventAny,
    orders::{Order, OrderAny},
    types::*,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
struct Memory {
    records: Arc<Mutex<Vec<Record>>>,
    fail: bool,
}
impl Store for Memory {
    fn save(&mut self, _: &str, record: &Record) -> Result<()> {
        ensure!(!self.fail, "Injected persistence failure");
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }
}
struct ObservedBroker {
    inner: MockBroker,
    records: Arc<Mutex<Vec<Record>>>,
    calls: Arc<AtomicUsize>,
    lose_ack: bool,
}
#[async_trait]
impl Broker for ObservedBroker {
    async fn verify(&self) -> Result<()> {
        Ok(())
    }
    async fn snapshot(&self) -> Result<Snapshot> {
        self.inner.snapshot().await
    }
    async fn execute(&self, command: &Command) -> Result<Outcome> {
        {
            let records = self.records.lock().unwrap();
            let last = records
                .last()
                .expect("ownership must be persisted before broker call");
            match command {
                Command::Place { tag, .. } => {
                    assert_eq!(last.outcome, "Dispatching");
                    assert_eq!(&last.tag, tag);
                    assert!(matches!(
                        last.events.last(),
                        Some(OrderEventAny::Submitted(_))
                    ));
                }
                Command::ProtectiveStopMarket { tag, .. } => {
                    assert_eq!(last.outcome, "Dispatching");
                    assert_eq!(&last.tag, tag);
                }
                Command::ModifyProtectiveStop { .. } => {
                    assert!(
                        last.management
                            .values()
                            .any(|v| v.contains("StopModify:") && v.ends_with("Dispatching"))
                    );
                }
                Command::Cancel { .. } => {
                    assert!(last.management.values().any(|s| s == "Dispatching"))
                }
                _ => panic!("unexpected modification"),
            }
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = self.inner.execute(command).await?;
        Ok(if self.lose_ack {
            Outcome::Unknown
        } else {
            result
        })
    }
}
fn order(side: OrderSide, qty: u64, reduce: bool) -> OrderAny {
    let mut f = OrderFactory::new(
        "SUSANTA-001".into(),
        "CROSSOVER-001".into(),
        None,
        None,
        Rc::new(RefCell::new(TestClock::new())),
        true,
        false,
    );
    f.limit(
        "CRUDEOIL26SEPFUT.MCX".into(),
        side,
        Quantity::from(qty),
        Price::from("6000"),
        Some(TimeInForce::Day),
        None,
        None,
        Some(reduce),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
}
fn fixture(fail: bool, lose_ack: bool) -> (Dispatcher, Arc<AtomicUsize>, Arc<Mutex<Vec<Record>>>) {
    let records = Arc::new(Mutex::new(vec![]));
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = Arc::new(ObservedBroker {
        inner: MockBroker::new(144870151, "NRML"),
        records: records.clone(),
        calls: calls.clone(),
        lose_ack,
    });
    let factory = OrderEventFactory::new(
        "SUSANTA-001".into(),
        "KITE-MOCK".into(),
        AccountType::Margin,
        Some(Currency::INR()),
    );
    (
        Dispatcher::new(
            broker,
            Box::new(Memory {
                records: records.clone(),
                fail,
            }),
            factory,
            "NRML".into(),
            144870151,
            "CRUDEOIL26SEPFUT.MCX".into(),
            "CRUDEOIL26SEPFUT".into(),
        ),
        calls,
        records,
    )
}
fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<ExecutionEvent>) -> Vec<OrderEventAny> {
    let mut events = vec![];
    while let Ok(e) = rx.try_recv() {
        if let ExecutionEvent::Order(e) = e {
            events.push(e)
        }
    }
    events
}
#[tokio::test]
async fn persistence_failure_blocks_before_broker_and_poison_prevents_retry() {
    let (mut d, calls, _) = fixture(true, false);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let o = order(OrderSide::Buy, 1, false);
    assert!(d.submit(o.clone(), 0, &tx).await.is_err());
    assert!(d.submit(o, 0, &tx).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(drain(&mut rx).is_empty());
}
#[tokio::test]
async fn acknowledged_receipt_waits_for_observation_and_duplicate_commands_are_not_retried() {
    let (mut d, calls, records) = fixture(false, false);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut o = order(OrderSide::Buy, 1, false);
    d.submit(o.clone(), 0, &tx).await.unwrap();
    let events = drain(&mut rx);
    assert!(matches!(&events[..], [OrderEventAny::Submitted(_)]));
    for e in events {
        o.apply(e).unwrap();
    }
    assert_eq!(
        records.lock().unwrap().last().unwrap().outcome,
        "Acknowledged"
    );
    d.refresh(&tx).await.unwrap();
    let events = drain(&mut rx);
    assert!(matches!(
        &events[..],
        [OrderEventAny::Accepted(_), OrderEventAny::Filled(_)]
    ));
    for e in events {
        o.apply(e).unwrap();
    }
    d.refresh(&tx).await.unwrap();
    assert!(drain(&mut rx).is_empty());
    assert!(d.submit(o, 1, &tx).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn lost_ack_is_correlated_from_persisted_tag_without_resubmission() {
    let (mut d, calls, records) = fixture(false, true);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    d.submit(order(OrderSide::Buy, 1, false), 0, &tx)
        .await
        .unwrap();
    assert_eq!(records.lock().unwrap().last().unwrap().outcome, "Unknown");
    drain(&mut rx);
    d.refresh(&tx).await.unwrap();
    assert_eq!(drain(&mut rx).len(), 2);
    assert_eq!(
        records.lock().unwrap().last().unwrap().broker_id.as_deref(),
        Some("1")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn reducing_exit_requires_matching_broker_position_and_cannot_reverse() {
    let (mut d, calls, _) = fixture(false, false);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    d.submit(order(OrderSide::Buy, 1, false), 0, &tx)
        .await
        .unwrap();
    d.refresh(&tx).await.unwrap();
    drain(&mut rx);
    for (qty, pos) in [(2, 1), (1, 0)] {
        d.submit(order(OrderSide::Sell, qty, true), pos, &tx)
            .await
            .unwrap();
        assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    d.submit(order(OrderSide::Sell, 1, true), 1, &tx)
        .await
        .unwrap();
    d.refresh(&tx).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(matches!(
        &drain(&mut rx)[..],
        [
            OrderEventAny::Submitted(_),
            OrderEventAny::Accepted(_),
            OrderEventAny::Filled(_)
        ]
    ));
}
#[tokio::test]
async fn cancellation_ack_is_not_canceled_until_observed_and_repeat_is_blocked() {
    let (mut d, calls, _) = fixture(false, false);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let o = order(OrderSide::Buy, 1, false);
    let id = o.client_order_id();
    d.submit(o, 0, &tx).await.unwrap();
    drain(&mut rx);
    d.cancel(id, UUID4::new(), &tx).await.unwrap();
    assert!(drain(&mut rx).is_empty());
    d.refresh(&tx).await.unwrap();
    assert!(matches!(
        &drain(&mut rx)[..],
        [OrderEventAny::Accepted(_), OrderEventAny::Canceled(_)]
    ));
    assert!(d.cancel(id, UUID4::new(), &tx).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn short_entry_cover_and_contract_cap_are_enforced_at_dispatch() {
    let (mut d, calls, _) = fixture(false, false);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    d.submit(order(OrderSide::Sell, 2, false), 0, &tx)
        .await
        .unwrap();
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    d.submit(order(OrderSide::Sell, 1, false), 0, &tx)
        .await
        .unwrap();
    d.refresh(&tx).await.unwrap();
    drain(&mut rx);
    d.submit(order(OrderSide::Sell, 1, false), -1, &tx)
        .await
        .unwrap();
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]));
    d.submit(order(OrderSide::Buy, 1, true), -1, &tx)
        .await
        .unwrap();
    d.refresh(&tx).await.unwrap();
    d.finish(&tx, false).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[derive(Clone, Copy)]
enum ObservationFault {
    PositionLag,
    TradeLag,
    WrongOwner,
    DuplicateTrade,
    SessionExpired,
    RateLimited,
}
struct LagBroker {
    inner: ObservedBroker,
    fault: ObservationFault,
    remaining: AtomicUsize,
    reads: AtomicUsize,
}
#[async_trait]
impl Broker for LagBroker {
    async fn verify(&self) -> Result<()> {
        Ok(())
    }
    async fn execute(&self, command: &Command) -> Result<Outcome> {
        self.inner.execute(command).await
    }
    async fn snapshot(&self) -> Result<Snapshot> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let mut snapshot = self.inner.snapshot().await?;
        if !snapshot.orders.is_empty()
            && self
                .remaining
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        {
            match self.fault {
                ObservationFault::PositionLag => {
                    let side = if snapshot.trades.last().unwrap().transaction_type == "BUY" {
                        1
                    } else {
                        -1
                    };
                    snapshot.positions[0].quantity -= side;
                    snapshot.positions[0].average_price = if snapshot.positions[0].quantity == 0 {
                        rust_decimal::Decimal::ZERO
                    } else {
                        rust_decimal::Decimal::from(6000)
                    };
                }
                ObservationFault::TradeLag => {
                    snapshot.trades.pop();
                }
                ObservationFault::WrongOwner => {
                    snapshot.orders.last_mut().unwrap().instrument_token = 1;
                }
                ObservationFault::DuplicateTrade => {
                    snapshot
                        .trades
                        .push(snapshot.trades.last().unwrap().clone());
                }
                ObservationFault::SessionExpired => {
                    return Err(anyhow::anyhow!(super::outage::ReadFailure::SessionExpired));
                }
                ObservationFault::RateLimited => {
                    return Err(anyhow::anyhow!(super::outage::ReadFailure::RateLimited(
                        10_000
                    )));
                }
            }
        }
        Ok(snapshot)
    }
}
fn lag_fixture(
    fault: ObservationFault,
    remaining: usize,
) -> (Dispatcher, Arc<LagBroker>, Arc<Mutex<Vec<Record>>>) {
    let records = Arc::new(Mutex::new(vec![]));
    let broker = Arc::new(LagBroker {
        inner: ObservedBroker {
            inner: MockBroker::new(144870151, "NRML"),
            records: records.clone(),
            calls: Arc::new(AtomicUsize::new(0)),
            lose_ack: false,
        },
        fault,
        remaining: AtomicUsize::new(remaining),
        reads: AtomicUsize::new(0),
    });
    let d = Dispatcher::new(
        broker.clone(),
        Box::new(Memory {
            records: records.clone(),
            fail: false,
        }),
        OrderEventFactory::new(
            "SUSANTA-001".into(),
            "KITE-MOCK".into(),
            AccountType::Margin,
            Some(Currency::INR()),
        ),
        "NRML".into(),
        144870151,
        "CRUDEOIL26SEPFUT.MCX".into(),
        "CRUDEOIL26SEPFUT".into(),
    );
    (d, broker, records)
}
#[tokio::test]
async fn reconciliation_retries_lagging_trade_and_position_reads_without_resubmitting() {
    for fault in [ObservationFault::PositionLag, ObservationFault::TradeLag] {
        for side in [OrderSide::Buy, OrderSide::Sell] {
            let (mut d, broker, _) = lag_fixture(fault, 2);
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            d.submit(order(side, 1, false), 0, &tx).await.unwrap();
            drain(&mut rx);
            d.refresh(&tx).await.unwrap();
            assert_eq!(broker.reads.load(Ordering::SeqCst), 4); // Preflight + three observations.
            assert_eq!(broker.inner.calls.load(Ordering::SeqCst), 1);
            assert!(matches!(
                &drain(&mut rx)[..],
                [OrderEventAny::Accepted(_), OrderEventAny::Filled(_)]
            ));
            d.refresh(&tx).await.unwrap();
            assert!(drain(&mut rx).is_empty());
            let (exit, position) = if side == OrderSide::Buy {
                (OrderSide::Sell, 1)
            } else {
                (OrderSide::Buy, -1)
            };
            d.submit(order(exit, 1, true), position, &tx).await.unwrap();
            drain(&mut rx);
            broker.remaining.store(2, Ordering::SeqCst);
            // Final shutdown reconciliation must also tolerate delayed exit observations.
            d.finish(&tx, false).await.unwrap();
            assert_eq!(broker.inner.calls.load(Ordering::SeqCst), 2);
            assert!(matches!(
                &drain(&mut rx)[..],
                [OrderEventAny::Accepted(_), OrderEventAny::Filled(_)]
            ));
            assert_eq!(d.unresolved(), 0);
        }
    }
}
#[tokio::test]
async fn reconciliation_persistent_lag_exhausts_reads_without_publishing_or_persisting_fills() {
    for fault in [ObservationFault::PositionLag, ObservationFault::TradeLag] {
        let (mut d, broker, records) = lag_fixture(fault, 99);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        d.submit(order(OrderSide::Buy, 1, false), 0, &tx)
            .await
            .unwrap();
        drain(&mut rx);
        let saved = records.lock().unwrap().len();
        assert!(d.refresh(&tx).await.is_err());
        assert_eq!(broker.reads.load(Ordering::SeqCst), 4);
        assert_eq!(records.lock().unwrap().len(), saved);
        assert_eq!(d.unresolved(), 1);
        assert!(drain(&mut rx).is_empty());
        assert_eq!(broker.inner.calls.load(Ordering::SeqCst), 1);
        d.fault();
        assert!(d.finish(&tx, false).await.is_err());
    }
}
#[tokio::test]
async fn reconciliation_integrity_auth_and_rate_errors_fail_without_retry() {
    for fault in [
        ObservationFault::WrongOwner,
        ObservationFault::DuplicateTrade,
        ObservationFault::SessionExpired,
        ObservationFault::RateLimited,
    ] {
        let (mut d, broker, records) = lag_fixture(fault, 99);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        d.submit(order(OrderSide::Buy, 1, false), 0, &tx)
            .await
            .unwrap();
        drain(&mut rx);
        let saved = records.lock().unwrap().len();
        assert!(d.refresh(&tx).await.is_err());
        assert_eq!(broker.reads.load(Ordering::SeqCst), 2);
        assert_eq!(records.lock().unwrap().len(), saved);
        assert!(drain(&mut rx).is_empty());
    }
}

fn stop_market_order() -> OrderAny {
    let mut f = OrderFactory::new(
        "SUSANTA-001".into(),
        "CROSSOVER-001".into(),
        None,
        None,
        Rc::new(RefCell::new(TestClock::new())),
        true,
        false,
    );
    f.stop_market(
        "CRUDEOIL26SEPFUT.MCX".into(),
        OrderSide::Sell,
        Quantity::from(1),
        Price::from("5900"),
        None,
        Some(TimeInForce::Day),
        None,
        Some(true),
        Some(false),
        None,
        None,
        None,
        None,
        None,
        None,
        Some("ILRCSTOP001".into()),
    )
}
#[tokio::test]
async fn persisted_stop_modification_requires_broker_observation_and_emits_updated() {
    let (mut d, calls, records) = fixture(false, false);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    d.submit(order(OrderSide::Buy, 1, false), 0, &tx)
        .await
        .unwrap();
    d.refresh(&tx).await.unwrap();
    drain(&mut rx);
    let stop = stop_market_order();
    d.submit(stop.clone(), 1, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OrderEventAny::Accepted(_)))
    );
    d.modify_stop(stop.client_order_id(), UUID4::new(), 6000)
        .await
        .unwrap();
    d.refresh(&tx).await.unwrap();
    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OrderEventAny::Updated(_)))
    );
    assert!(
        records
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .management
            .is_empty()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

/// SATS `exchange_stop_loss` flow: entry, resting SL-M, an exit is refused while
/// the stop can still fill, the cancel must be observed, then the exit goes out.
#[tokio::test]
async fn resting_stop_blocks_exits_until_its_cancel_is_observed() {
    let (mut d, calls, _) = fixture(false, false);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    // a protective stop without a position is denied before the broker
    d.submit(stop_market_order(), 0, &tx).await.unwrap();
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // entry fills
    d.submit(order(OrderSide::Buy, 1, false), 0, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    drain(&mut rx);
    // SL-M rests at the broker
    let stop = stop_market_order();
    d.submit(stop.clone(), 1, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    assert!(drain(&mut rx).iter().any(|e| matches!(e, OrderEventAny::Accepted(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // an exit while the stop is working is denied without reaching the broker
    d.submit(order(OrderSide::Sell, 1, true), 1, &tx).await.unwrap();
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // cancel: acknowledged first, Canceled only once the broker shows it
    d.cancel(stop.client_order_id(), UUID4::new(), &tx).await.unwrap();
    assert!(drain(&mut rx).is_empty());
    d.refresh(&tx).await.unwrap();
    assert!(drain(&mut rx).iter().any(|e| matches!(e, OrderEventAny::Canceled(_))));
    // now the exit is admitted and fills; the run finishes clean
    d.submit(order(OrderSide::Sell, 1, true), 1, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    assert!(drain(&mut rx).iter().any(|e| matches!(e, OrderEventAny::Filled(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    d.finish(&tx, false).await.unwrap();
}
#[tokio::test]
async fn multi_lot_orders_need_the_reviewed_max_lots_cap() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    // default cap is one lot: a 3-lot entry never reaches the broker
    let (mut one, calls, _) = fixture(false, false);
    one.submit(order(OrderSide::Buy, 3, false), 0, &tx).await.unwrap();
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // with max_lots 3: enter 3, scale out 1, refuse an exit larger than the position, close 2
    let (d, calls, _) = fixture(false, false);
    let mut d = d.with_max_lots(3);
    d.submit(order(OrderSide::Buy, 3, false), 0, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    assert!(drain(&mut rx).iter().any(|e| matches!(e, OrderEventAny::Filled(_))));
    d.submit(order(OrderSide::Buy, 1, false), 3, &tx).await.unwrap();
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]), "no adding to an open position");
    d.submit(order(OrderSide::Sell, 1, true), 3, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    assert!(drain(&mut rx).iter().any(|e| matches!(e, OrderEventAny::Filled(_))));
    d.submit(order(OrderSide::Sell, 3, true), 2, &tx).await.unwrap();
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]), "exit larger than the position would reverse");
    d.submit(order(OrderSide::Sell, 2, true), 2, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    assert!(drain(&mut rx).iter().any(|e| matches!(e, OrderEventAny::Filled(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    d.finish(&tx, false).await.unwrap();
    // above the cap even with max_lots
    let (d, calls, _) = fixture(false, false);
    let mut d = d.with_max_lots(3);
    d.submit(order(OrderSide::Buy, 4, false), 0, &tx).await.unwrap();
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn single_order_flip_closes_and_reverses_within_the_cap() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let (d, calls, _) = fixture(false, false);
    let mut d = d.with_max_lots(3);
    let filled = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<ExecutionEvent>| {
        drain(rx).iter().any(|e| matches!(e, OrderEventAny::Filled(_)))
    };
    d.submit(order(OrderSide::Buy, 3, false), 0, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    assert!(filled(&mut rx));
    // no TP hit: +3 → −3 is one SELL 6
    d.submit(order(OrderSide::Sell, 6, false), 3, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    assert!(filled(&mut rx), "flip SELL 6 fills");
    // TP1 on the short (−3 → −2), then flip after TP1: −2 → +3 is one BUY 5
    d.submit(order(OrderSide::Buy, 1, true), -3, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    assert!(filled(&mut rx));
    d.submit(order(OrderSide::Buy, 5, false), -2, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    assert!(filled(&mut rx), "flip BUY 5 after TP1 fills");
    // refused before the broker: flip beyond the cap (+3 → −4), a non-reduce order that
    // only closes (+3 → 0) or only reduces (+3 → +1), and adding on the same side
    for (side, qty) in [(OrderSide::Sell, 7), (OrderSide::Sell, 3), (OrderSide::Sell, 2), (OrderSide::Buy, 1)] {
        d.submit(order(side, qty, false), 3, &tx).await.unwrap();
        assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]), "{side:?} {qty} must be denied");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 4, "denied orders never reach the broker");
    d.submit(order(OrderSide::Sell, 3, true), 3, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    assert!(filled(&mut rx));
    d.finish(&tx, false).await.unwrap();
}

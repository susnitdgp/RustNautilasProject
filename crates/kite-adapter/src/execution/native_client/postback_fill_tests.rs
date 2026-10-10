//! Postback fast path (kite-adapter 0.3.1): a COMPLETE order postback reads that order
//! and its trades and emits the fill with the real Kite trade ID; the full snapshot that
//! follows must confirm it trade for trade within the grace period.
use super::super::{
    broker_events::{BrokerOrder, BrokerTrade},
    request::Command,
    transport::Outcome,
};
use super::{
    broker::{Broker, Snapshot},
    dispatch::Dispatcher,
    ledger::{Record, Store},
    mock::MockBroker,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use nautilus_common::{
    clock::TestClock,
    factories::{OrderEventFactory, OrderFactory},
    messages::ExecutionEvent,
};
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
        atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    },
    time::Duration,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

struct Memory(Arc<Mutex<Vec<Record>>>);
impl Store for Memory {
    fn save(&mut self, _: &str, record: &Record) -> Result<()> {
        self.0.lock().unwrap().push(record.clone());
        Ok(())
    }
}

/// Mock broker whose full snapshot can lag or disagree with the per-order reads.
struct Lagging {
    inner: MockBroker,
    /// Full snapshot: `/trades` does not show the fill yet.
    hide_trades: AtomicBool,
    /// Full snapshot: positions still flat.
    flat_positions: AtomicBool,
    /// Full snapshot: the trade price differs from the per-order read.
    reprice: AtomicBool,
    /// Per-order reads unsupported (`Ok(None)`), like the mock and test brokers.
    unsupported: AtomicBool,
    detail_reads: AtomicUsize,
}
#[async_trait]
impl Broker for Lagging {
    async fn verify(&self) -> Result<()> {
        Ok(())
    }
    async fn execute(&self, command: &Command) -> Result<Outcome> {
        self.inner.execute(command).await
    }
    async fn snapshot(&self) -> Result<Snapshot> {
        let mut s = self.inner.snapshot().await?;
        if self.hide_trades.load(SeqCst) {
            s.trades.clear();
        }
        if self.flat_positions.load(SeqCst) {
            for p in &mut s.positions {
                p.quantity = 0;
            }
        }
        if self.reprice.load(SeqCst) {
            for t in &mut s.trades {
                t.average_price += rust_decimal::Decimal::ONE;
            }
        }
        Ok(s)
    }
    async fn order_detail(
        &self,
        order_id: &str,
    ) -> Result<Option<(BrokerOrder, Vec<BrokerTrade>)>> {
        if self.unsupported.load(SeqCst) {
            return Ok(None);
        }
        self.detail_reads.fetch_add(1, SeqCst);
        let s = self.inner.snapshot().await?;
        let order = s
            .orders
            .into_iter()
            .find(|o| o.order_id == order_id)
            .ok_or_else(|| anyhow!("unknown order"))?;
        let trades = s
            .trades
            .into_iter()
            .filter(|t| t.order_id == order_id)
            .collect();
        Ok(Some((order, trades)))
    }
}

fn order(side: OrderSide) -> OrderAny {
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
        Quantity::from(1),
        Price::from("6000"),
        Some(TimeInForce::Day),
        None,
        None,
        Some(false),
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

struct Fixture {
    d: Dispatcher,
    broker: Arc<Lagging>,
    records: Arc<Mutex<Vec<Record>>>,
    tx: UnboundedSender<ExecutionEvent>,
    rx: UnboundedReceiver<ExecutionEvent>,
}
fn fixture(inner: MockBroker, grace: Duration) -> Fixture {
    let records = Arc::new(Mutex::new(vec![]));
    let broker = Arc::new(Lagging {
        inner,
        hide_trades: AtomicBool::new(false),
        flat_positions: AtomicBool::new(false),
        reprice: AtomicBool::new(false),
        unsupported: AtomicBool::new(false),
        detail_reads: AtomicUsize::new(0),
    });
    let factory = OrderEventFactory::new(
        "SUSANTA-001".into(),
        "KITE-MOCK".into(),
        AccountType::Margin,
        Some(Currency::INR()),
    );
    let mut d = Dispatcher::new(
        broker.clone(),
        Box::new(Memory(records.clone())),
        factory,
        "NRML".into(),
        144870151,
        "CRUDEOIL26SEPFUT.MCX".into(),
        "CRUDEOIL26SEPFUT".into(),
    )
    .with_verify_grace(grace);
    // Off by default since 2.22.0; these tests exercise the path itself.
    d.set_postback_fast_fill(true);
    let (tx, rx) = unbounded_channel();
    Fixture {
        d,
        broker,
        records,
        tx,
        rx,
    }
}
fn drain(rx: &mut UnboundedReceiver<ExecutionEvent>) -> Vec<OrderEventAny> {
    let mut events = vec![];
    while let Ok(e) = rx.try_recv() {
        if let ExecutionEvent::Order(e) = e {
            events.push(e)
        }
    }
    events
}
/// Submits a BUY 1 from flat; returns the native order (Submitted) and its Kite order id.
async fn submitted(f: &mut Fixture) -> (OrderAny, String) {
    let mut o = order(OrderSide::Buy);
    f.d.submit(o.clone(), 0, &f.tx).await.unwrap();
    for e in drain(&mut f.rx) {
        o.apply(e).unwrap();
    }
    let id = f.records.lock().unwrap().last().unwrap().broker_id.clone().unwrap();
    (o, id)
}

#[tokio::test]
async fn fast_path_is_off_by_default_and_reads_nothing() {
    let mut f = fixture(MockBroker::new(144870151, "NRML"), Duration::from_secs(30));
    f.d.set_postback_fast_fill(false);
    let (_, id) = submitted(&mut f).await;
    let before = f.records.lock().unwrap().len();
    assert!(!f.d.fast_fill(&id, &f.tx).await.unwrap());
    assert_eq!(f.broker.detail_reads.load(SeqCst), 0, "no REST reads when off");
    assert!(drain(&mut f.rx).is_empty());
    assert_eq!(f.records.lock().unwrap().len(), before);
    assert!(!f.d.needs_refresh() || f.d.has_unresolved());
    // the full reconciliation fills it as before
    f.d.refresh(&f.tx).await.unwrap();
    assert!(matches!(
        &drain(&mut f.rx)[..],
        [OrderEventAny::Accepted(_), OrderEventAny::Filled(_)]
    ));
}

#[tokio::test]
async fn complete_postback_emits_the_real_trade_fill_and_the_snapshot_confirms_it() {
    let mut f = fixture(MockBroker::new(144870151, "NRML"), Duration::from_secs(30));
    let (mut o, id) = submitted(&mut f).await;
    assert!(f.d.fast_fill(&id, &f.tx).await.unwrap());
    let events = drain(&mut f.rx);
    assert!(matches!(
        &events[..],
        [OrderEventAny::Accepted(_), OrderEventAny::Filled(_)]
    ));
    // journalled before emission, and keyed by the broker's own trade id
    let journalled = f.records.lock().unwrap().last().unwrap().clone();
    assert_eq!(journalled.outcome, "Observed");
    assert!(journalled.events.iter().any(|e| matches!(e, OrderEventAny::Filled(_))));
    let OrderEventAny::Filled(fill) = &events[1] else { unreachable!() };
    let broker_trades = f.broker.inner.snapshot().await.unwrap().trades;
    assert_eq!(broker_trades.len(), 1);
    assert_eq!(fill.trade_id.as_str(), broker_trades[0].trade_id);
    for e in events {
        o.apply(e).unwrap();
    }
    assert_eq!(o.status(), OrderStatus::Filled);
    assert!(!f.d.has_unresolved() && f.d.needs_refresh());
    // the full snapshot confirms the same trade: no duplicate fill, verification done
    f.d.refresh(&f.tx).await.unwrap();
    assert!(drain(&mut f.rx).is_empty());
    assert!(!f.d.needs_refresh());
    // a repeated postback for the closed order reads nothing and emits nothing
    assert!(!f.d.fast_fill(&id, &f.tx).await.unwrap());
    assert_eq!(f.broker.detail_reads.load(SeqCst), 1);
    assert!(drain(&mut f.rx).is_empty());
}

#[tokio::test]
async fn snapshot_lagging_inside_the_grace_period_is_tolerated_then_verified() {
    let mut f = fixture(MockBroker::new(144870151, "NRML"), Duration::from_secs(30));
    let (_, id) = submitted(&mut f).await;
    f.broker.hide_trades.store(true, SeqCst);
    f.broker.flat_positions.store(true, SeqCst);
    assert!(f.d.fast_fill(&id, &f.tx).await.unwrap());
    assert_eq!(drain(&mut f.rx).len(), 2);
    // neither /trades nor positions show the fill yet: no error, no events, keep checking
    f.d.refresh(&f.tx).await.unwrap();
    assert!(drain(&mut f.rx).is_empty());
    assert!(f.d.needs_refresh());
    f.broker.hide_trades.store(false, SeqCst);
    f.broker.flat_positions.store(false, SeqCst);
    f.d.refresh(&f.tx).await.unwrap();
    assert!(drain(&mut f.rx).is_empty());
    assert!(!f.d.needs_refresh());
}

#[tokio::test]
async fn fill_still_missing_after_the_grace_period_stops_for_review() {
    let mut f = fixture(MockBroker::new(144870151, "NRML"), Duration::from_millis(1));
    let (_, id) = submitted(&mut f).await;
    f.broker.hide_trades.store(true, SeqCst);
    assert!(f.d.fast_fill(&id, &f.tx).await.unwrap());
    tokio::time::sleep(Duration::from_millis(20)).await;
    let error = f.d.refresh(&f.tx).await.unwrap_err().to_string();
    assert!(error.contains("not confirmed"), "{error}");
}

#[tokio::test]
async fn snapshot_trade_differing_from_the_postback_fill_is_an_integrity_failure() {
    let mut f = fixture(MockBroker::new(144870151, "NRML"), Duration::from_secs(30));
    let (_, id) = submitted(&mut f).await;
    assert!(f.d.fast_fill(&id, &f.tx).await.unwrap());
    drain(&mut f.rx);
    f.broker.reprice.store(true, SeqCst);
    let error = f.d.refresh(&f.tx).await.unwrap_err().to_string();
    assert!(error.contains("changed after processing"), "{error}");
}

#[tokio::test]
async fn order_not_complete_yet_falls_back_without_any_change() {
    // first read sees the order still OPEN
    let mut f = fixture(
        MockBroker::new(144870151, "NRML").delayed(1),
        Duration::from_secs(30),
    );
    let (_, id) = submitted(&mut f).await;
    let before = f.records.lock().unwrap().len();
    assert!(!f.d.fast_fill(&id, &f.tx).await.unwrap());
    assert!(drain(&mut f.rx).is_empty());
    assert_eq!(f.records.lock().unwrap().len(), before);
    assert!(f.d.has_unresolved());
    // the next postback finds it COMPLETE
    assert!(f.d.fast_fill(&id, &f.tx).await.unwrap());
    assert_eq!(drain(&mut f.rx).len(), 2);
}

#[tokio::test]
async fn unsupported_or_unknown_orders_take_the_full_reconciliation() {
    let mut f = fixture(MockBroker::new(144870151, "NRML"), Duration::from_secs(30));
    let (_, id) = submitted(&mut f).await;
    // a manual order's postback: not owned, nothing read
    assert!(!f.d.fast_fill("999999", &f.tx).await.unwrap());
    f.broker.unsupported.store(true, SeqCst);
    assert!(!f.d.fast_fill(&id, &f.tx).await.unwrap());
    assert_eq!(f.broker.detail_reads.load(SeqCst), 0);
    assert!(drain(&mut f.rx).is_empty());
    // the existing path still fills it
    f.d.refresh(&f.tx).await.unwrap();
    assert!(matches!(
        &drain(&mut f.rx)[..],
        [OrderEventAny::Accepted(_), OrderEventAny::Filled(_)]
    ));
    assert!(!f.d.needs_refresh());
}

/// Shapes from the Kite Connect v3 orders documentation: a day-book entry (`GET /orders`)
/// and an order's trades (`GET /orders/{id}/trades`) parse into the structs `reconcile`
/// checks, with the fields the fast path depends on.
#[test]
fn documented_day_book_and_order_trades_parse() {
    let order: BrokerOrder = serde_json::from_str(
        r#"{"placed_by":"XXXXXX","order_id":"700000000000000","exchange_order_id":"800000000000000",
        "parent_order_id":null,"status":"COMPLETE","status_message":null,"status_message_raw":null,
        "order_timestamp":"2021-05-31 16:00:36","exchange_update_timestamp":"2021-05-31 16:00:36",
        "exchange_timestamp":"2021-05-31 16:00:36","variety":"regular","modified":false,
        "exchange":"MCX","tradingsymbol":"GOLDPETAL21JUNFUT","instrument_token":58424839,
        "order_type":"LIMIT","transaction_type":"BUY","validity":"DAY","product":"NRML",
        "quantity":1,"disclosed_quantity":0,"price":4854,"trigger_price":0,"average_price":4852,
        "filled_quantity":1,"pending_quantity":0,"cancelled_quantity":0,"market_protection":0,
        "meta":{},"tag":"connect test order1","tags":["connect test order1"],"guid":"XXXXXXX"}"#,
    )
    .unwrap();
    assert_eq!(order.status, "COMPLETE");
    assert_eq!(order.filled_quantity, 1);
    assert_eq!(order.market_protection, Some(rust_decimal::Decimal::ZERO));
    assert_eq!(
        order.exchange_update_timestamp.as_deref(),
        Some("2021-05-31 16:00:36")
    );
    let trades: Vec<BrokerTrade> = serde_json::from_str(
        r#"[{"trade_id":"10000000","order_id":"200000000000000","exchange":"MCX",
        "tradingsymbol":"GOLDPETAL21JUNFUT","instrument_token":58424839,"product":"NRML",
        "average_price":4852,"quantity":1,"exchange_order_id":"300000000000000",
        "transaction_type":"BUY","fill_timestamp":"2021-05-31 16:00:36",
        "order_timestamp":"16:00:36","exchange_timestamp":"2021-05-31 16:00:36"}]"#,
    )
    .unwrap();
    assert_eq!(trades[0].trade_id, "10000000");
    assert_eq!(trades[0].quantity, 1);
    assert_eq!(trades[0].fill_timestamp, "2021-05-31 16:00:36");
}

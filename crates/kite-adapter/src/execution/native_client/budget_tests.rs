//! Order-rate budget failures (kite-adapter 0.5.0): an entry is denied, an exit still goes
//! out, and neither stops the run. Until 0.5.0 both faulted the dispatcher, so an open
//! position could no longer be closed.
use super::super::{request::Command, transport::Outcome};
use super::{
    broker::{Broker, Snapshot},
    dispatch::Dispatcher,
    ledger::{Record, Store},
    mock::MockBroker,
};
use anyhow::{Result, bail};
use async_trait::async_trait;
use nautilus_common::{
    clock::TestClock,
    factories::{OrderEventFactory, OrderFactory},
    messages::ExecutionEvent,
};
use nautilus_model::{enums::*, events::OrderEventAny, orders::OrderAny, types::*};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    },
};

/// A store whose order-rate budget can be switched to failing.
struct Budget(Arc<AtomicBool>);
impl Store for Budget {
    fn save(&mut self, _: &str, _: &Record) -> Result<()> {
        Ok(())
    }
    fn reserve(&mut self) -> Result<()> {
        if self.0.load(SeqCst) {
            bail!("order-rate budget unavailable (Redis); will reconnect")
        }
        Ok(())
    }
}
struct Counting {
    inner: MockBroker,
    calls: AtomicUsize,
}
#[async_trait]
impl Broker for Counting {
    async fn verify(&self) -> Result<()> {
        Ok(())
    }
    async fn snapshot(&self) -> Result<Snapshot> {
        self.inner.snapshot().await
    }
    async fn execute(&self, command: &Command) -> Result<Outcome> {
        self.calls.fetch_add(1, SeqCst);
        self.inner.execute(command).await
    }
}
fn order(side: OrderSide, reduce: bool) -> OrderAny {
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
async fn budget_failure_denies_entries_and_lets_exits_through_without_stopping_the_run() {
    let failing = Arc::new(AtomicBool::new(false));
    let broker = Arc::new(Counting {
        inner: MockBroker::new(144870151, "NRML"),
        calls: AtomicUsize::new(0),
    });
    let mut d = Dispatcher::new(
        broker.clone(),
        Box::new(Budget(failing.clone())),
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // 1. budget down: a new entry is denied, nothing reaches the broker, the run goes on
    failing.store(true, SeqCst);
    d.submit(order(OrderSide::Buy, false), 0, &tx).await.unwrap();
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]));
    assert_eq!(broker.calls.load(SeqCst), 0);

    // 2. budget back: the entry goes through and fills
    failing.store(false, SeqCst);
    d.submit(order(OrderSide::Buy, false), 0, &tx).await.unwrap();
    d.refresh(&tx).await.unwrap();
    assert!(drain(&mut rx).iter().any(|e| matches!(e, OrderEventAny::Filled(_))));
    assert_eq!(broker.calls.load(SeqCst), 1);

    // 3. budget down again: the exit (reduce-only) still goes out and fills
    failing.store(true, SeqCst);
    d.submit(order(OrderSide::Sell, true), 1, &tx).await.unwrap();
    assert_eq!(broker.calls.load(SeqCst), 2, "exit reached the broker");
    d.refresh(&tx).await.unwrap();
    let events = drain(&mut rx);
    assert!(matches!(events.first(), Some(OrderEventAny::Submitted(_))));
    assert!(events.iter().any(|e| matches!(e, OrderEventAny::Filled(_))));
    assert!(!d.has_unresolved());
}

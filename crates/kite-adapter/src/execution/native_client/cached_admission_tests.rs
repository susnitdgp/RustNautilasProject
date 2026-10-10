//! Cached admission (kite-adapter 0.2.9): orders are admitted from the last clean
//! reconciliation while it is fresh; anything else falls back to the full REST preflight.
use super::super::{request::Command, transport::Outcome};
use super::{
    broker::{Broker, Snapshot},
    dispatch::{CACHED_ADMISSION_MAX_AGE, Dispatcher},
    ledger::{Record, Store, expire_previous_journals},
    mock::MockBroker,
};
use anyhow::Result;
use async_trait::async_trait;
use nautilus_common::{
    clock::TestClock,
    factories::{OrderEventFactory, OrderFactory},
    messages::ExecutionEvent,
};
use nautilus_model::{
    enums::*,
    events::OrderEventAny,
    orders::OrderAny,
    types::*,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
#[path = "../../../../kite-journal/test-support/redis.rs"]
mod support;

struct Memory;
impl Store for Memory {
    fn save(&mut self, _: &str, _: &Record) -> Result<()> {
        Ok(())
    }
}
/// Counts REST snapshots and broker mutations; can add an unowned open order.
struct Counting {
    inner: MockBroker,
    snapshots: Arc<AtomicUsize>,
    executes: Arc<AtomicUsize>,
    foreign_order: Arc<AtomicBool>,
}
#[async_trait]
impl Broker for Counting {
    async fn verify(&self) -> Result<()> {
        Ok(())
    }
    async fn snapshot(&self) -> Result<Snapshot> {
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        let mut s = self.inner.snapshot().await?;
        if self.foreign_order.load(Ordering::SeqCst)
            && let Some(mut manual) = s.orders.first().cloned()
        {
            manual.order_id = "999999".into();
            manual.tag = None;
            manual.status = "OPEN".into();
            manual.filled_quantity = 0;
            s.orders.push(manual);
        }
        Ok(s)
    }
    async fn execute(&self, command: &Command) -> Result<Outcome> {
        self.executes.fetch_add(1, Ordering::SeqCst);
        self.inner.execute(command).await
    }
}
struct Fixture {
    d: Dispatcher,
    snapshots: Arc<AtomicUsize>,
    executes: Arc<AtomicUsize>,
    foreign_order: Arc<AtomicBool>,
}
fn fixture(cached: bool) -> Fixture {
    let snapshots = Arc::new(AtomicUsize::new(0));
    let executes = Arc::new(AtomicUsize::new(0));
    let foreign_order = Arc::new(AtomicBool::new(false));
    let broker = Arc::new(Counting {
        inner: MockBroker::new(144870151, "NRML"),
        snapshots: snapshots.clone(),
        executes: executes.clone(),
        foreign_order: foreign_order.clone(),
    });
    let factory = OrderEventFactory::new(
        "SUSANTA-001".into(),
        "KITE-MOCK".into(),
        AccountType::Margin,
        Some(Currency::INR()),
    );
    let d = Dispatcher::new(
        broker,
        Box::new(Memory),
        factory,
        "NRML".into(),
        144870151,
        "CRUDEOIL26SEPFUT.MCX".into(),
        "CRUDEOIL26SEPFUT".into(),
    )
    .with_cached_admission(cached);
    Fixture {
        d,
        snapshots,
        executes,
        foreign_order,
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
fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<ExecutionEvent>) -> Vec<OrderEventAny> {
    let mut events = vec![];
    while let Ok(e) = rx.try_recv() {
        if let ExecutionEvent::Order(e) = e {
            events.push(e)
        }
    }
    events
}
/// Long 1 after an entry admitted by the full preflight and a clean reconciliation.
async fn long_one(f: &mut Fixture, ready: &AtomicBool) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    f.d.submit_guarded(order(OrderSide::Buy, 1, false), 0, &tx, Some(ready))
        .await
        .unwrap();
    f.d.refresh(&tx).await.unwrap();
    let events = drain(&mut rx);
    assert!(events.iter().any(|e| matches!(e, OrderEventAny::Filled(_))));
    assert_eq!(f.d.admissions(), (0, 1));
}

#[tokio::test]
async fn fresh_clean_reconciliation_admits_without_rest() {
    let mut f = fixture(true);
    let ready = AtomicBool::new(true);
    long_one(&mut f, &ready).await;
    let (snapshots, executes) = (f.snapshots.load(Ordering::SeqCst), f.executes.load(Ordering::SeqCst));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    f.d.submit_guarded(order(OrderSide::Sell, 1, true), 1, &tx, Some(&ready))
        .await
        .unwrap();
    assert_eq!(f.snapshots.load(Ordering::SeqCst), snapshots, "no REST read");
    assert_eq!(f.executes.load(Ordering::SeqCst), executes + 1, "order sent");
    assert_eq!(f.d.admissions(), (1, 1));
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Submitted(_)]));
}

#[tokio::test]
async fn startup_observation_admits_the_first_entry() {
    let mut f = fixture(true);
    let ready = AtomicBool::new(true);
    f.d.mark_clean(Instant::now(), f.d.doorbell().load(Ordering::SeqCst));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    f.d.submit_guarded(order(OrderSide::Buy, 1, false), 0, &tx, Some(&ready))
        .await
        .unwrap();
    assert_eq!(f.snapshots.load(Ordering::SeqCst), 0);
    assert_eq!(f.d.admissions(), (1, 0));
}

#[tokio::test]
async fn order_update_after_the_observation_forces_rest_and_catches_manual_order() {
    let mut f = fixture(true);
    let ready = AtomicBool::new(true);
    long_one(&mut f, &ready).await;
    // A manual order arrives: the stream bumps the doorbell before REST reflects it.
    f.foreign_order.store(true, Ordering::SeqCst);
    f.d.doorbell().fetch_add(1, Ordering::SeqCst);
    let snapshots = f.snapshots.load(Ordering::SeqCst);
    let executes = f.executes.load(Ordering::SeqCst);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    f.d.submit_guarded(order(OrderSide::Sell, 1, true), 1, &tx, Some(&ready))
        .await
        .unwrap();
    assert!(f.snapshots.load(Ordering::SeqCst) > snapshots, "REST preflight ran");
    assert_eq!(f.executes.load(Ordering::SeqCst), executes, "nothing sent");
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]));
    assert_eq!(f.d.admissions(), (0, 1));
}

#[tokio::test]
async fn stale_observation_forces_rest() {
    let mut f = fixture(true);
    let ready = AtomicBool::new(true);
    let Some(old) = Instant::now().checked_sub(CACHED_ADMISSION_MAX_AGE + Duration::from_secs(1))
    else {
        return; // monotonic clock too close to boot to build a stale instant
    };
    f.d.mark_clean(old, f.d.doorbell().load(Ordering::SeqCst));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    f.d.submit_guarded(order(OrderSide::Buy, 1, false), 0, &tx, Some(&ready))
        .await
        .unwrap();
    assert_eq!(f.snapshots.load(Ordering::SeqCst), 1);
    assert_eq!(f.d.admissions(), (0, 1));
}

#[tokio::test]
async fn native_position_mismatch_forces_rest_and_is_denied() {
    let mut f = fixture(true);
    let ready = AtomicBool::new(true);
    long_one(&mut f, &ready).await;
    let snapshots = f.snapshots.load(Ordering::SeqCst);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    // The strategy believes it is flat; the observation says long 1.
    f.d.submit_guarded(order(OrderSide::Buy, 1, false), 0, &tx, Some(&ready))
        .await
        .unwrap();
    assert!(f.snapshots.load(Ordering::SeqCst) > snapshots);
    assert!(matches!(&drain(&mut rx)[..], [OrderEventAny::Denied(_)]));
}

#[tokio::test]
async fn sent_order_invalidates_the_observation_until_the_next_reconciliation() {
    let mut f = fixture(true);
    let ready = AtomicBool::new(true);
    f.d.mark_clean(Instant::now(), 0);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    f.d.submit_guarded(order(OrderSide::Buy, 1, false), 0, &tx, Some(&ready))
        .await
        .unwrap();
    assert_eq!(f.d.admissions(), (1, 0));
    // Without a reconciliation the order is unresolved: the exit is denied, never cached.
    f.d.submit_guarded(order(OrderSide::Sell, 1, true), 1, &tx, Some(&ready))
        .await
        .unwrap();
    assert_eq!(f.d.admissions(), (1, 0));
    f.d.refresh(&tx).await.unwrap();
    f.d.submit_guarded(order(OrderSide::Sell, 1, true), 1, &tx, Some(&ready))
        .await
        .unwrap();
    assert_eq!(f.d.admissions(), (2, 0));
}

#[tokio::test]
async fn disabled_or_unguarded_dispatch_always_reads_rest() {
    for cached in [false, true] {
        let mut f = fixture(cached);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        f.d.mark_clean(Instant::now(), 0);
        // `submit` passes no stream gate (mock/sandbox), so never cached.
        f.d.submit(order(OrderSide::Buy, 1, false), 0, &tx).await.unwrap();
        assert_eq!(f.snapshots.load(Ordering::SeqCst), 1);
        assert_eq!(f.d.admissions(), (0, 1));
    }
    let mut f = fixture(false);
    let ready = AtomicBool::new(true);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    f.d.mark_clean(Instant::now(), 0);
    f.d.submit_guarded(order(OrderSide::Buy, 1, false), 0, &tx, Some(&ready))
        .await
        .unwrap();
    assert_eq!(f.snapshots.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn fault_clears_the_observation() {
    let mut f = fixture(true);
    f.d.mark_clean(Instant::now(), 0);
    f.d.fault();
    let ready = AtomicBool::new(true);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    assert!(
        f.d.submit_guarded(order(OrderSide::Buy, 1, false), 0, &tx, Some(&ready))
            .await
            .is_err()
    );
    assert_eq!(f.executes.load(Ordering::SeqCst), 0);
}

#[test]
fn previous_run_journals_get_a_retention_ttl_once() {
    let server = support::TestRedis::new();
    let mut c = server.connection();
    let base = "kite-prod:v1:{slot-a}:commands:";
    for ns in ["20261009-aaaa", "20261009-bbbb", "20261010-cccc"] {
        let _: () = redis::cmd("HSET")
            .arg(format!("{base}{ns}"))
            .arg("scope")
            .arg("x")
            .query(&mut c)
            .unwrap();
    }
    // Another slot and an unrelated key are never touched.
    let _: () = redis::cmd("HSET")
        .arg("kite-prod:v1:{slot-b}:commands:20261009-dddd")
        .arg("scope")
        .arg("x")
        .query(&mut c)
        .unwrap();
    let current = format!("{base}20261010-cccc");
    assert_eq!(expire_previous_journals(&mut c, &current, "20261010-cccc").unwrap(), 2);
    assert_eq!(expire_previous_journals(&mut c, &current, "20261010-cccc").unwrap(), 0);
    let ttl = |c: &mut redis::Connection, k: &str| -> i64 {
        redis::cmd("TTL").arg(k).query(c).unwrap()
    };
    assert!(ttl(&mut c, &format!("{base}20261009-aaaa")) > 29 * 86400);
    assert_eq!(ttl(&mut c, &current), -1, "current run keeps no TTL");
    assert_eq!(ttl(&mut c, "kite-prod:v1:{slot-b}:commands:20261009-dddd"), -1);
}

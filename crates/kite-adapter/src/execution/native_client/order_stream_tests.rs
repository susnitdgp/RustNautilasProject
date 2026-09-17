use super::*;
use crate::execution::{
    native_client::{
        broker::{Broker, Snapshot},
        ledger::{Record, Store},
        mock::MockBroker,
    },
    request::Command,
    transport::Outcome,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use nautilus_common::{
    clock::TestClock,
    factories::{OrderEventFactory, OrderFactory},
};
use nautilus_model::{
    enums::{AccountType, OrderSide, TimeInForce},
    events::OrderEventAny,
    orders::OrderAny,
    types::{Currency, Price},
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Mutex as StdMutex, atomic::AtomicUsize},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tokio_tungstenite::{WebSocketStream, accept_async};

struct Memory(Arc<StdMutex<Vec<Record>>>);
impl Store for Memory {
    fn save(&mut self, _: &str, record: &Record) -> Result<()> {
        self.0.lock().unwrap().push(record.clone());
        Ok(())
    }
}
struct Observed {
    broker: MockBroker,
    reads: AtomicUsize,
    writes: AtomicUsize,
    fail: AtomicBool,
    delay: AtomicBool,
    entered: AtomicBool,
    release: Notify,
}
#[async_trait]
impl Broker for Observed {
    async fn verify(&self) -> Result<()> {
        Ok(())
    }
    async fn snapshot(&self) -> Result<Snapshot> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.delay.swap(false, Ordering::AcqRel) {
            self.entered.store(true, Ordering::Release);
            self.release.notified().await;
        }
        ensure!(
            !self.fail.load(Ordering::Acquire),
            "Injected reconciliation failure"
        );
        self.broker.snapshot().await
    }
    async fn execute(&self, command: &Command) -> Result<Outcome> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.broker.execute(command).await
    }
}
struct Fixture {
    monitor: Monitor,
    broker: Arc<Observed>,
    rx: mpsc::UnboundedReceiver<ExecutionEvent>,
    records: Arc<StdMutex<Vec<Record>>>,
}
fn fixture() -> Fixture {
    let broker = Arc::new(Observed {
        broker: MockBroker::new(144870151, "MIS"),
        reads: AtomicUsize::new(0),
        writes: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
        delay: AtomicBool::new(false),
        entered: AtomicBool::new(false),
        release: Notify::new(),
    });
    let records = Arc::new(StdMutex::new(vec![]));
    let (tx, rx) = mpsc::unbounded_channel();
    let dispatcher = Dispatcher::new(
        broker.clone(),
        Box::new(Memory(records.clone())),
        OrderEventFactory::new(
            "SUSANTA-001".into(),
            "KITE-MOCK".into(),
            AccountType::Margin,
            Some(Currency::INR()),
        ),
        "MIS".into(),
        144870151,
        "CRUDEOIL26SEPFUT.MCX".into(),
        "CRUDEOIL26SEPFUT".into(),
    );
    Fixture {
        monitor: Monitor {
            dispatcher: Arc::new(Mutex::new(dispatcher)),
            tx,
            active: Arc::new(AtomicBool::new(true)),
            ready: Arc::new(AtomicBool::new(true)),
            credentials: Arc::new(
                KiteCredentials::new(Some("TEST".into()), Some("TEST".into())).unwrap(),
            ),
            user_id: "MOCK".into(),
        },
        broker,
        rx,
        records,
    }
}
fn order() -> OrderAny {
    let mut factory = OrderFactory::new(
        "SUSANTA-001".into(),
        "CROSSOVER-001".into(),
        None,
        None,
        Rc::new(RefCell::new(TestClock::new())),
        true,
        false,
    );
    factory.limit(
        "CRUDEOIL26SEPFUT.MCX".into(),
        OrderSide::Buy,
        1.into(),
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
async fn pair(
    credentials: &KiteCredentials,
) -> (
    transport::Socket,
    WebSocketStream<TcpStream>,
    TcpListener,
    String,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let (client, server) = tokio::join!(
        transport::connect_at(
            credentials,
            Instant::now() + Duration::from_secs(2),
            &endpoint
        ),
        async {
            let (tcp, _) = listener.accept().await.unwrap();
            accept_async(tcp).await.unwrap()
        }
    );
    (client.unwrap(), server, listener, endpoint)
}
fn timing() -> Timing {
    Timing {
        fallback: Duration::from_secs(30),
        pending: Duration::from_secs(30),
        idle: Duration::from_secs(3),
        reconnect: Duration::from_millis(20),
    }
}
async fn until(mut predicate: impl FnMut() -> bool) {
    timeout(Duration::from_secs(2), async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
fn update() -> Message {
    Message::Text(
        r#"{"type":"order","data":{"user_id":"MOCK","order_id":"1","status":"COMPLETE"}}"#.into(),
    )
}

#[test]
fn validates_account_and_redacts_bad_payloads() {
    assert!(
        is_order(
            r#"{"type":"order","data":{"user_id":"MOCK","order_id":"1","status":"UPDATE"}}"#,
            "MOCK"
        )
        .unwrap()
    );
    assert!(!is_order(r#"{"type":"message","data":"notice"}"#, "MOCK").unwrap());
    for payload in [
        r#"{"type":"order","data":{"user_id":"OTHER","order_id":"1","status":"COMPLETE","secret":"sentinel"}}"#,
        r#"{"type":"order","data":{"secret":"sentinel"}}"#,
        r#"{"type":"error","data":"sentinel"}"#,
        "sentinel",
    ] {
        assert!(
            !is_order(payload, "MOCK")
                .unwrap_err()
                .to_string()
                .contains("sentinel")
        );
    }
}

#[tokio::test]
async fn websocket_update_confirms_persisted_fill_once_without_resubmission() {
    let mut f = fixture();
    f.monitor
        .dispatcher
        .lock()
        .await
        .submit(order(), 0, &f.monitor.tx)
        .await
        .unwrap();
    assert!(matches!(
        f.rx.recv().await,
        Some(ExecutionEvent::Order(OrderEventAny::Submitted(_)))
    ));
    assert!(f.rx.try_recv().is_err()); // HTTP acknowledgement alone did not fill.
    let (socket, mut server, _listener, endpoint) = pair(&f.monitor.credentials).await;
    let active = f.monitor.active.clone();
    let task = tokio::spawn(async move { f.monitor.run_at(socket, &endpoint, timing()).await });
    server.send(update()).await.unwrap();
    let mut fills = 0;
    timeout(Duration::from_secs(2), async {
        loop {
            if let Some(ExecutionEvent::Order(OrderEventAny::Filled(_))) = f.rx.recv().await {
                fills += 1;
                assert!(
                    f.records
                        .lock()
                        .unwrap()
                        .last()
                        .unwrap()
                        .events
                        .iter()
                        .any(|e| matches!(e, OrderEventAny::Filled(_)))
                );
                break;
            }
        }
    })
    .await
    .unwrap();
    let before = f.broker.reads.load(Ordering::SeqCst);
    for _ in 0..20 {
        server.send(update()).await.unwrap();
    }
    until(|| f.broker.reads.load(Ordering::SeqCst) > before).await;
    active.store(false, Ordering::Release);
    task.await.unwrap().unwrap();
    while let Ok(ExecutionEvent::Order(e)) = f.rx.try_recv() {
        if matches!(e, OrderEventAny::Filled(_)) {
            fills += 1;
        }
    }
    assert_eq!(fills, 1);
    assert_eq!(f.broker.writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn idle_connection_uses_fallback_not_one_second_polling_and_handles_ping() {
    let f = fixture();
    let (socket, mut server, _listener, endpoint) = pair(&f.monitor.credentials).await;
    let active = f.monitor.active.clone();
    let mut times = timing();
    times.fallback = Duration::from_millis(250);
    times.pending = Duration::from_millis(40);
    let task = tokio::spawn(async move { f.monitor.run_at(socket, &endpoint, times).await });
    server.send(Message::Ping(vec![7].into())).await.unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(1), server.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Message::Pong(_)
    ));
    sleep(Duration::from_millis(120)).await;
    assert_eq!(f.broker.reads.load(Ordering::SeqCst), 0);
    until(|| f.broker.reads.load(Ordering::SeqCst) >= 1).await;
    active.store(false, Ordering::Release);
    task.await.unwrap().unwrap();
    assert_eq!(Timing::default().fallback, Duration::from_secs(15));
}

#[tokio::test]
async fn missing_order_update_is_recovered_by_pending_order_check() {
    let mut f = fixture();
    f.monitor
        .dispatcher
        .lock()
        .await
        .submit(order(), 0, &f.monitor.tx)
        .await
        .unwrap();
    let (socket, _server, _listener, endpoint) = pair(&f.monitor.credentials).await;
    let active = f.monitor.active.clone();
    let mut times = timing();
    times.pending = Duration::from_millis(50);
    let task = tokio::spawn(async move { f.monitor.run_at(socket, &endpoint, times).await });
    timeout(Duration::from_secs(2), async {
        loop {
            if matches!(
                f.rx.recv().await,
                Some(ExecutionEvent::Order(OrderEventAny::Filled(_)))
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    active.store(false, Ordering::Release);
    task.await.unwrap().unwrap();
    assert_eq!(f.broker.writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reconnect_reconciles_before_reopening_admission() {
    let f = fixture();
    let (socket, server, listener, endpoint) = pair(&f.monitor.credentials).await;
    let active = f.monitor.active.clone();
    let ready = f.monitor.ready.clone();
    let task = tokio::spawn(async move { f.monitor.run_at(socket, &endpoint, timing()).await });
    drop(server);
    until(|| !ready.load(Ordering::Acquire)).await;
    let (tcp, _) = timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    // TCP alone cannot reopen admission: websocket handshake and reconciliation are required.
    assert!(!ready.load(Ordering::Acquire));
    let _replacement = accept_async(tcp).await.unwrap();
    until(|| ready.load(Ordering::Acquire)).await;
    assert!(f.broker.reads.load(Ordering::SeqCst) >= 1);
    active.store(false, Ordering::Release);
    task.await.unwrap().unwrap();
    assert!(!ready.load(Ordering::Acquire));
}

#[tokio::test]
async fn heartbeat_timeout_reconnects_and_stop_interrupts_handshake() {
    let f = fixture();
    let (socket, _silent_server, listener, endpoint) = pair(&f.monitor.credentials).await;
    let active = f.monitor.active.clone();
    let ready = f.monitor.ready.clone();
    let mut times = timing();
    times.idle = Duration::from_millis(50);
    let task = tokio::spawn(async move { f.monitor.run_at(socket, &endpoint, times).await });
    until(|| !ready.load(Ordering::Acquire)).await;
    // Accept TCP but deliberately never answer the upgrade request.
    let (_tcp, _) = timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    active.store(false, Ordering::Release);
    timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn websocket_error_and_failed_reconciliation_clear_admission() {
    for fail_reads in [false, true] {
        let f = fixture();
        let (socket, mut server, _listener, endpoint) = pair(&f.monitor.credentials).await;
        let ready = f.monitor.ready.clone();
        f.broker.fail.store(fail_reads, Ordering::Release);
        let task = tokio::spawn(async move { f.monitor.run_at(socket, &endpoint, timing()).await });
        server
            .send(if fail_reads {
                update()
            } else {
                Message::Text(r#"{"type":"error","data":"private-sentinel"}"#.into())
            })
            .await
            .unwrap();
        let error = timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(!error.to_string().contains("private-sentinel"));
        assert!(!ready.load(Ordering::Acquire));
    }
}

#[tokio::test]
async fn disconnect_during_preflight_denies_entry_before_broker_write() {
    let mut f = fixture();
    f.broker.delay.store(true, Ordering::Release);
    let dispatcher = f.monitor.dispatcher.clone();
    let ready = f.monitor.ready.clone();
    let tx = f.monitor.tx.clone();
    let o = order();
    let task = tokio::spawn(async move {
        dispatcher
            .lock()
            .await
            .submit_guarded(o, 0, &tx, Some(&ready))
            .await
    });
    until(|| f.broker.entered.load(Ordering::Acquire)).await;
    f.monitor.ready.store(false, Ordering::Release);
    f.broker.release.notify_one();
    task.await.unwrap().unwrap();
    assert_eq!(f.broker.writes.load(Ordering::SeqCst), 0);
    assert!(f.records.lock().unwrap().is_empty());
    assert!(matches!(
        f.rx.recv().await,
        Some(ExecutionEvent::Order(OrderEventAny::Denied(_)))
    ));
}

#[tokio::test]
async fn old_snapshot_cannot_reopen_admission_after_reconnect() {
    let f = fixture();
    let (socket, mut server, listener, endpoint) = pair(&f.monitor.credentials).await;
    let active = f.monitor.active.clone();
    let ready = f.monitor.ready.clone();
    f.broker.delay.store(true, Ordering::Release);
    let task = tokio::spawn(async move { f.monitor.run_at(socket, &endpoint, timing()).await });
    server.send(update()).await.unwrap();
    until(|| f.broker.entered.load(Ordering::Acquire)).await;
    drop(server);
    until(|| !ready.load(Ordering::Acquire)).await;
    let (tcp, _) = timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let _replacement = accept_async(tcp).await.unwrap();
    sleep(Duration::from_millis(20)).await;
    assert!(!ready.load(Ordering::Acquire));
    f.broker.release.notify_one();
    until(|| ready.load(Ordering::Acquire)).await;
    assert!(f.broker.reads.load(Ordering::SeqCst) >= 2);
    active.store(false, Ordering::Release);
    task.await.unwrap().unwrap();
}

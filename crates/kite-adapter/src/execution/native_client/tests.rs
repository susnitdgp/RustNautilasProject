use super::broker::{BrokerPosition, Funds, Utilised};
use super::*;
use nautilus_common::{
    clock::TestClock, factories::OrderFactory, live::runner::replace_exec_event_sender,
};
use nautilus_core::UUID4;
use nautilus_model::orders::Order;
use rust_decimal::Decimal;
use std::{
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};
struct Mock {
    snapshot: Snapshot,
    calls: Arc<AtomicUsize>,
    verified: bool,
}
#[async_trait]
impl Broker for Mock {
    async fn verify(&self) -> Result<()> {
        ensure!(self.verified, "Mock account mismatch");
        Ok(())
    }
    async fn snapshot(&self) -> Result<Snapshot> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.snapshot.clone())
    }
}
fn config() -> Config {
    Config {
        user_id: "TEST123".into(),
        product: "NRML".into(),
        instrument_token: 144870151,
        credentials: Arc::new(
            crate::credentials::KiteCredentials::new(
                Some("test-key".into()),
                Some("test-token".into()),
            )
            .unwrap(),
        ),
    }
}
fn snapshot() -> Snapshot {
    Snapshot {
        orders: vec![],
        trades: vec![],
        positions: vec![],
        funds: Funds {
            enabled: true,
            net: Decimal::from(9000),
            utilised: Utilised {
                debits: Decimal::from(1000),
            },
        },
    }
}
fn client(
    s: Snapshot,
    verified: bool,
) -> (
    Client,
    Arc<AtomicUsize>,
    tokio::sync::mpsc::UnboundedReceiver<ExecutionEvent>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    replace_exec_event_sender(tx);
    let calls = Arc::new(AtomicUsize::new(0));
    let client = Client::new(
        "SUSANTA-001".into(),
        "KITE",
        config(),
        Box::new(Mock {
            snapshot: s,
            calls: calls.clone(),
            verified,
        }),
    )
    .unwrap();
    (client, calls, rx)
}
fn native_order() -> OrderAny {
    let mut f = OrderFactory::new(
        "SUSANTA-001".into(),
        "CROSSOVER-001".into(),
        None,
        None,
        Rc::new(RefCell::new(TestClock::new())),
        false,
        false,
    );
    f.limit(
        "CRUDEOIL26SEPFUT.MCX".into(),
        OrderSide::Buy,
        Quantity::from(1),
        Price::from("6000"),
        Some(TimeInForce::Day),
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
    )
}
#[tokio::test]
async fn native_connect_emits_reported_resources_and_submission_is_denied_without_broker_call() {
    let (mut c, calls, mut rx) = client(snapshot(), true);
    c.start().unwrap();
    assert!(!c.is_connected());
    c.connect().await.unwrap();
    assert!(c.is_connected());
    match rx.try_recv().unwrap() {
        ExecutionEvent::Account(a) => {
            assert_eq!(a.account_id, AccountId::from("KITE-TEST123"));
            assert!(a.is_reported);
            assert_eq!(a.balances[0].total, Money::new(10000.0, Currency::INR()));
            assert_eq!(a.balances[0].free, Money::new(9000.0, Currency::INR()));
            assert_eq!(a.balances[0].locked, Money::new(1000.0, Currency::INR()));
        }
        _ => panic!("account expected"),
    }
    assert!(c.get_account().is_some());
    let mut order = native_order();
    let cmd = SubmitOrder::from_order(
        &order,
        "SUSANTA-001".into(),
        Some("KITE".into()),
        None,
        UUID4::new(),
        Client::now(),
    );
    c.submit_order(cmd).unwrap();
    match rx.try_recv().unwrap() {
        ExecutionEvent::Order(e @ OrderEventAny::Denied(_)) => order.apply(e).unwrap(),
        _ => panic!("denial expected"),
    }
    assert_eq!(order.status(), OrderStatus::Denied);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    c.disconnect().await.unwrap();
    c.stop().unwrap();
    c.dispose().unwrap();
    assert!(!c.is_connected());
}
#[tokio::test]
async fn account_mismatch_prevents_snapshot_and_connection() {
    let (mut c, calls, _) = client(snapshot(), false);
    assert!(c.connect().await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!c.is_connected());
}
#[tokio::test]
async fn native_position_report_is_explicitly_flat_and_rejects_historical_or_other_product_coverage()
 {
    let (mut c, _, _rx) = client(snapshot(), true);
    c.connect().await.unwrap();
    let cmd = GeneratePositionStatusReportsBuilder::default()
        .ts_init(Client::now())
        .build()
        .unwrap();
    let p = c.generate_position_status_reports(&cmd).await.unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].position_side, PositionSide::Flat);
    assert!(!c.provides_bulk_position_coverage("CRUDEOIL26SEPFUT.MCX".into()));
    let mut historical = cmd;
    historical.start = Some(1.into());
    assert!(
        c.generate_position_status_reports(&historical)
            .await
            .is_err()
    );
    let mut s = snapshot();
    s.positions.push(BrokerPosition {
        exchange: "MCX".into(),
        tradingsymbol: "CRUDEOIL26SEPFUT".into(),
        instrument_token: 144870151,
        product: "MIS".into(),
        quantity: 1,
        average_price: Decimal::from(6000),
    });
    let (mut c, _, _rx) = client(s, true);
    assert!(c.connect().await.is_err());
}
#[tokio::test]
async fn fill_reports_refuse_to_invent_missing_commission() {
    let mut s = snapshot();
    s.trades.push(serde_json::from_str(r#"{"trade_id":"456","order_id":"123","exchange":"MCX","tradingsymbol":"CRUDEOIL26SEPFUT","instrument_token":144870151,"product":"NRML","transaction_type":"BUY","quantity":1,"average_price":6000,"fill_timestamp":"2026-09-15 10:00:00"}"#).unwrap());
    s.orders.push(super::super::broker_events::BrokerOrder {
        order_id: "123".into(),
        exchange: "MCX".into(),
        tradingsymbol: "CRUDEOIL26SEPFUT".into(),
        instrument_token: 144870151,
        product: "NRML".into(),
        transaction_type: "BUY".into(),
        variety: "regular".into(),
        order_type: "LIMIT".into(),
        market_protection: None,
        validity: "DAY".into(),
        status: "COMPLETE".into(),
        quantity: 1,
        filled_quantity: 1,
        price: Decimal::from(6000),
        tag: None,
        exchange_timestamp: Some("2026-09-15 10:00:00".into()),
        exchange_update_timestamp: Some("2026-09-15 10:00:00".into()),
        order_timestamp: "2026-09-15 10:00:00".into(),
    });
    let (mut c, _, _rx) = client(s, true);
    c.connect().await.unwrap();
    let cmd = GenerateFillReportsBuilder::default()
        .ts_init(Client::now())
        .build()
        .unwrap();
    assert!(
        c.generate_fill_reports(cmd)
            .await
            .unwrap_err()
            .to_string()
            .contains("commissions")
    );
    assert!(c.generate_mass_status(None).await.is_err());
}
#[tokio::test]
async fn native_broker_order_queries_preserve_status_and_do_not_guess_client_ownership() {
    let mut s = snapshot();
    s.orders.push(serde_json::from_str(r#"{"order_id":"123","exchange":"MCX","tradingsymbol":"CRUDEOIL26SEPFUT","instrument_token":144870151,"product":"NRML","transaction_type":"BUY","variety":"regular","order_type":"LIMIT","validity":"DAY","status":"OPEN","quantity":1,"filled_quantity":0,"price":6000,"tag":"Native1","exchange_timestamp":"2026-09-15 10:00:01","exchange_update_timestamp":"2026-09-15 10:00:02","order_timestamp":"2026-09-15 10:00:00"}"#).unwrap());
    let (mut c, _, _rx) = client(s, true);
    c.connect().await.unwrap();
    let cmd = GenerateOrderStatusReportsBuilder::default()
        .ts_init(Client::now())
        .open_only(true)
        .build()
        .unwrap();
    let reports = c.generate_order_status_reports(&cmd).await.unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].order_status, OrderStatus::Accepted);
    assert!(reports[0].client_order_id.is_none());
    let single = GenerateOrderStatusReportBuilder::default()
        .ts_init(Client::now())
        .venue_order_id(Some("123".into()))
        .build()
        .unwrap();
    assert!(
        c.generate_order_status_report(&single)
            .await
            .unwrap()
            .is_some()
    );
    let mut other = single;
    other.client_order_id = Some("unowned".into());
    assert!(c.generate_order_status_report(&other).await.is_err());
}
#[test]
fn registers_through_native_execution_factory_without_connecting_or_loading_credentials() {
    use nautilus_common::{cache::Cache, factories::ExecutionClientFactoryRegistry};
    let mut registry = ExecutionClientFactoryRegistry::new();
    registry.register("KITE".into(), Box::new(Factory)).unwrap();
    let cache = Rc::new(RefCell::new(Cache::default()));
    let c = registry
        .get("KITE")
        .unwrap()
        .create("SUSANTA-001".into(), "KITE", &config(), cache.into())
        .unwrap();
    assert_eq!(c.account_id(), AccountId::from("KITE-TEST123"));
    assert!(!c.is_connected());
    let mut bad = config();
    bad.product = "CNC".into();
    assert!(bad.validate().is_err());
}

#[tokio::test]
async fn native_mass_reconciliation_preserves_owned_ids_fills_and_positions() {
    struct Records(Vec<super::ledger::Record>);
    impl super::ledger::Store for Records {
        fn save(&mut self, _: &str, r: &super::ledger::Record) -> Result<()> {
            self.0.push(r.clone());
            Ok(())
        }
    }
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    replace_exec_event_sender(tx.clone());
    let mut c = Client::new(
        "SUSANTA-001".into(),
        "KITE",
        config(),
        Box::new(super::mock::MockBroker::new(144870151, "NRML")),
    )
    .unwrap();
    c.connect().await.unwrap();
    c.dispatcher = Some(Arc::new(tokio::sync::Mutex::new(
        super::dispatch::Dispatcher::new(
            c.broker.clone(),
            Box::new(Records(vec![])),
            c.factory.clone(),
            "NRML".into(),
            144870151,
        ),
    )));
    let order = native_order();
    let id = order.client_order_id();
    {
        let mut d = c.dispatcher.as_ref().unwrap().lock().await;
        d.submit(order, 0, &tx).await.unwrap();
        d.refresh(&tx).await.unwrap();
    }
    let report = c.generate_mass_status(None).await.unwrap().unwrap();
    let orders = report.order_reports();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders.values().next().unwrap().client_order_id, Some(id));
    let fills = report.fill_reports();
    assert_eq!(fills.len(), 1);
    let fill = &fills.values().next().unwrap()[0];
    assert_eq!(fill.client_order_id, Some(id));
    assert_eq!(fill.commission, Money::new(0.0, Currency::INR()));
    assert_eq!(fill.last_qty, Quantity::from(1));
    let positions = report.position_reports();
    assert_eq!(
        positions.values().next().unwrap()[0].position_side,
        PositionSide::Long
    );
    let query = GenerateOrderStatusReportBuilder::default()
        .ts_init(Client::now())
        .client_order_id(Some(id))
        .build()
        .unwrap();
    assert_eq!(
        c.generate_order_status_report(&query)
            .await
            .unwrap()
            .unwrap()
            .venue_order_id,
        VenueOrderId::from("1")
    );
    assert!(c.generate_mass_status(Some(u64::MAX)).await.is_err());
    assert!(
        c.disconnect()
            .await
            .unwrap_err()
            .to_string()
            .contains("review")
    );
}

#[tokio::test]
async fn native_polling_delivers_delayed_fill_without_resubmitting() {
    struct Records;
    impl super::ledger::Store for Records {
        fn save(&mut self, _: &str, _: &super::ledger::Record) -> Result<()> {
            Ok(())
        }
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    replace_exec_event_sender(tx);
    let mut c = Client::new(
        "SUSANTA-001".into(),
        "KITE",
        config(),
        Box::new(super::mock::MockBroker::new(144870151, "NRML").delayed(2)),
    )
    .unwrap();
    c.cache = Some(Rc::new(RefCell::new(nautilus_common::cache::Cache::default())).into());
    c.dispatcher = Some(Arc::new(tokio::sync::Mutex::new(
        super::dispatch::Dispatcher::new(
            c.broker.clone(),
            Box::new(Records),
            c.factory.clone(),
            "NRML".into(),
            144870151,
        ),
    )));
    c.connect().await.unwrap();
    let order = native_order();
    c.submit_order(SubmitOrder::from_order(
        &order,
        "SUSANTA-001".into(),
        Some("KITE".into()),
        None,
        UUID4::new(),
        Client::now(),
    ))
    .unwrap();
    let observed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut events = vec![];
        while let Some(event) = rx.recv().await {
            if let ExecutionEvent::Order(e) = event {
                let filled = matches!(e, OrderEventAny::Filled(_));
                events.push(e);
                if filled {
                    return events;
                }
            }
        }
        panic!("Native event channel closed")
    })
    .await
    .unwrap();
    assert!(matches!(
        &observed[..],
        [
            OrderEventAny::Submitted(_),
            OrderEventAny::Accepted(_),
            OrderEventAny::Filled(_)
        ]
    ));
    let mut reconstructed = order;
    for event in observed {
        reconstructed.apply(event).unwrap();
    }
    assert_eq!(reconstructed.status(), OrderStatus::Filled);
    assert!(
        c.disconnect()
            .await
            .unwrap_err()
            .to_string()
            .contains("review")
    );
    assert!(!c.is_connected());
}

use kite_execution::reports::{batch, verification};
use kite_journal::{
    actions::{Action, Change},
    model::{Event, Intent, Product, Side},
    store::Journal,
};
use nautilus_model::{
    enums::{LiquiditySide, OrderStatus},
    identifiers::VenueOrderId,
    reports::ExecutionMassStatus,
    types::Quantity,
};
use rust_decimal::Decimal;
#[path = "../../kite-journal/test-support/redis.rs"]
mod support;
use support::TestRedis;
fn prepare(j: &mut Journal, id: &str, product: Product) {
    j.append(Event::Intent {
        intent: Intent {
            id: id.into(),
            symbol: "CRUDEOIL26SEPFUT".into(),
            side: Side::Buy,
            product,
            quantity: 2,
            limit_price_paise: 600100,
        },
    })
    .unwrap();
    j.append(Event::Dispatch { id: id.into() }).unwrap();
}
fn setup(server: &TestRedis, name: &str) -> Journal {
    let mut j = Journal::create_at(&server.url, name).unwrap();
    prepare(&mut j, "Order", Product::Nrml);
    j.append(Event::Acknowledged {
        id: "Order".into(),
        broker_id: "Broker1".into(),
    })
    .unwrap();
    j
}
fn fill(j: &mut Journal, trade: &str, price: i64) {
    j.append(Event::Fill {
        id: "Order".into(),
        broker_id: "Broker1".into(),
        trade_id: trade.into(),
        quantity: 1,
        price_paise: price,
    })
    .unwrap();
}
#[test]
fn native_simulation_roundtrips_and_preserves_identical_reports() {
    let server = TestRedis::new();
    let result = verification::run_at(&server.url, "check").unwrap();
    assert_eq!(result.native_order_reports, 3);
    assert_eq!(result.native_fill_reports, 3);
    assert!(
        result.replay_reports_identical
            && result.native_serialization_roundtrip
            && result.oms_ack_not_venue_acceptance
    );
    assert!(!result.reports_complete && !result.execution_engine_started);
}
#[test]
fn exact_prices_quantities_average_and_local_timestamps() {
    let server = TestRedis::new();
    let mut j = setup(&server, "test");
    fill(&mut j, "T1", 600000);
    fill(&mut j, "T2", 600100);
    let b = batch::build(&j).unwrap();
    let orders = b.native.order_reports();
    let o = &orders[&VenueOrderId::from("Broker1")];
    assert_eq!(o.quantity, Quantity::from(2));
    assert_eq!(o.filled_qty, Quantity::from(2));
    assert_eq!(o.avg_px, Some(Decimal::new(600050, 2)));
    assert_eq!(o.order_status, OrderStatus::Filled);
    assert_eq!(o.ts_accepted.as_u64(), 0);
    assert_eq!(
        o.ts_last.as_u64(),
        j.history().last().unwrap().recorded_at_ns
    );
    let fills = b.native.fill_reports();
    let f = &fills[&VenueOrderId::from("Broker1")];
    assert_eq!(f.len(), 2);
    assert_eq!(f[0].last_qty, Quantity::from(1));
    assert_eq!(f[0].last_px.as_decimal(), Decimal::from(6000));
    assert_eq!(f[0].liquidity_side, LiquiditySide::NoLiquiditySide);
    assert_eq!(f[0].commission.as_decimal(), Decimal::ZERO);
    assert_eq!(f[0].ts_event.as_u64(), j.history()[3].recorded_at_ns);
}
#[test]
fn unresolved_management_is_not_emitted_as_confirmed_state() {
    let server = TestRedis::new();
    let mut j = setup(&server, "test");
    fill(&mut j, "T1", 600000);
    j.append(Event::Management {
        id: "Order".into(),
        action: Action::Prepare {
            command_id: "M1".into(),
            change: Change::Modify {
                quantity: 3,
                limit_price_paise: 600100,
            },
        },
    })
    .unwrap();
    j.append(Event::Management {
        id: "Order".into(),
        action: Action::Dispatch {
            command_id: "M1".into(),
        },
    })
    .unwrap();
    let b = batch::build(&j).unwrap();
    assert_eq!(b.skipped_orders, 1);
    assert!(b.native.order_reports().is_empty());
    assert_eq!(
        b.native
            .fill_reports()
            .values()
            .map(Vec::len)
            .sum::<usize>(),
        1
    );
    assert!(b.issues.contains("order_requires_reconciliation"));
    assert!(!b.native.reports_complete());
}
#[test]
fn unknown_orders_have_no_invented_broker_id_and_products_stay_separate() {
    let server = TestRedis::new();
    let mut j = setup(&server, "test");
    prepare(&mut j, "Unknown", Product::Mis);
    j.append(Event::Unknown {
        id: "Unknown".into(),
    })
    .unwrap();
    let b = batch::build(&j).unwrap();
    assert_eq!(b.native.order_reports().len(), 1);
    assert_eq!(b.skipped_orders, 1);
    assert_eq!(b.product_by_client["Order"], Product::Nrml);
    assert_eq!(b.product_by_client["Unknown"], Product::Mis);
    assert!(b.native.position_reports().is_empty());
}
#[test]
fn duplicate_fill_does_not_change_report_ids_and_new_generation_is_distinct() {
    let server = TestRedis::new();
    let mut j = setup(&server, "test");
    fill(&mut j, "T1", 600000);
    let before = batch::build(&j).unwrap();
    let count = j.record_count();
    fill(&mut j, "T1", 600000);
    assert_eq!(j.record_count(), count);
    assert_eq!(before.native, batch::build(&j).unwrap().native);
    let other = setup(&server, "other");
    assert_ne!(
        before.native.report_id,
        batch::build(&other).unwrap().native.report_id
    );
    let encoded = serde_json::to_string(&before.native).unwrap();
    assert_eq!(
        serde_json::from_str::<ExecutionMassStatus>(&encoded).unwrap(),
        before.native
    );
}
#[test]
fn uncertain_journal_write_blocks_report_export() {
    let server = TestRedis::new();
    let mut j = setup(&server, "test");
    let mut c = server.connection();
    redis::cmd("DEL")
        .arg(Journal::key("test").unwrap())
        .query::<()>(&mut c)
        .unwrap();
    assert!(
        j.append(Event::Unknown {
            id: "missing".into()
        })
        .is_err()
    );
    // Force an actual Redis write failure after valid local validation.
    assert!(
        j.append(Event::Cancelled {
            id: "Order".into(),
            broker_id: "Broker1".into()
        })
        .is_err()
    );
    assert!(batch::build(&j).is_err());
}
#[test]
fn journal_clock_regression_requires_review() {
    let server = TestRedis::new();
    let j = setup(&server, "test");
    drop(j);
    let mut c = server.connection();
    let key = Journal::key("test").unwrap();
    let raw: String = redis::cmd("HGET")
        .arg(&key)
        .arg("event:3")
        .query(&mut c)
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    value["recorded_at_ns"] = serde_json::json!(1);
    redis::cmd("HSET")
        .arg(&key)
        .arg("event:3")
        .arg(value.to_string())
        .query::<()>(&mut c)
        .unwrap();
    let j = Journal::open_at(&server.url, "test").unwrap();
    assert!(batch::build(&j).is_err());
}

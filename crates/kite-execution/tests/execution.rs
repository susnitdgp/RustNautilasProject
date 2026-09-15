use kite_execution::{
    coordinator,
    mock::{MockBroker, Outcome},
    translation, verification,
};
use kite_journal::{
    model::{Event, Intent, Product, Side},
    state::Status,
    store::Journal,
};
#[path = "../../kite-journal/test-support/redis.rs"]
mod support;
use support::TestRedis;

fn intent(id: &str) -> Intent {
    Intent {
        id: id.into(),
        symbol: "CRUDEOIL26SEPFUT".into(),
        side: Side::Buy,
        product: Product::Nrml,
        quantity: 2,
        limit_price_paise: 600000,
    }
}
fn prepare(j: &mut Journal, id: &str) {
    j.append(Event::Intent { intent: intent(id) }).unwrap();
}
fn dispatch(j: &mut Journal, id: &str) {
    j.append(Event::Dispatch { id: id.into() }).unwrap();
}
fn fill(id: &str, trade: &str, quantity: u32) -> Event {
    Event::Fill {
        id: id.into(),
        broker_id: "Broker1".into(),
        trade_id: trade.into(),
        quantity,
        price_paise: 600000,
    }
}
#[test]
fn simulation_reopens_and_blocks_ambiguous_and_interrupted_retries() {
    let dir = TestRedis::new();
    let path = dir.namespace();
    let result = verification::run_at(&dir.url, &path).unwrap();
    assert_eq!(result.simulated_submissions, 2);
    assert_eq!(result.filled_contracts, 2);
    assert_eq!(result.journal_records, 10);
    assert!(!result.broker_accessed && !result.live_orders_enabled);
    assert!(verification::run_at(&dir.url, &path).is_err());
}
#[test]
fn early_fills_late_ack_and_timeout_never_regress_state() {
    let dir = TestRedis::new();
    let path = dir.namespace();
    let mut j = Journal::create_at(&dir.url, &path).unwrap();
    prepare(&mut j, "Early");
    dispatch(&mut j, "Early");
    j.append(fill("Early", "T1", 1)).unwrap();
    assert!(
        !j.append(Event::Acknowledged {
            id: "Early".into(),
            broker_id: "Broker1".into()
        })
        .unwrap()
    );
    assert!(!j.append(Event::Unknown { id: "Early".into() }).unwrap());
    assert_eq!(
        j.state().order("Early").unwrap().status,
        Status::PartiallyFilled
    );
    j.append(Event::Cancelled {
        id: "Early".into(),
        broker_id: "Broker1".into(),
    })
    .unwrap();
    j.append(fill("Early", "T2", 1)).unwrap();
    assert_eq!(j.state().order("Early").unwrap().status, Status::Filled);
    drop(j);
    let j = Journal::open_at(&dir.url, &path).unwrap();
    assert_eq!(j.state().order("Early").unwrap().filled, 2);
}
#[test]
fn invalid_events_do_not_change_durable_or_memory_state() {
    let dir = TestRedis::new();
    let path = dir.namespace();
    let mut j = Journal::create_at(&dir.url, &path).unwrap();
    prepare(&mut j, "Order1");
    assert!(j.append(fill("Order1", "T1", 1)).is_err());
    dispatch(&mut j, "Order1");
    j.append(fill("Order1", "T1", 1)).unwrap();
    let count = j.record_count();
    assert!(!j.append(fill("Order1", "T1", 1)).unwrap());
    assert!(j.append(fill("Order1", "T1", 2)).is_err());
    assert!(j.append(fill("Order1", "T2", 2)).is_err());
    assert!(
        j.append(Event::Rejected {
            id: "Order1".into()
        })
        .is_err()
    );
    assert_eq!(j.record_count(), count);
    assert_eq!(j.state().order("Order1").unwrap().filled, 1);
    drop(j);
    assert_eq!(
        Journal::open_at(&dir.url, &path).unwrap().record_count(),
        count
    );
}
#[test]
fn duplicate_intents_and_broker_identity_collisions_are_blocked() {
    let dir = TestRedis::new();
    let path = dir.namespace();
    let mut j = Journal::create_at(&dir.url, &path).unwrap();
    prepare(&mut j, "One");
    prepare(&mut j, "Two");
    assert!(
        j.append(Event::Intent {
            intent: intent("One")
        })
        .is_err()
    );
    dispatch(&mut j, "One");
    dispatch(&mut j, "Two");
    j.append(Event::Acknowledged {
        id: "One".into(),
        broker_id: "Broker1".into(),
    })
    .unwrap();
    assert!(
        j.append(Event::Acknowledged {
            id: "Two".into(),
            broker_id: "Broker1".into()
        })
        .is_err()
    );
    assert!(
        j.append(Event::Acknowledged {
            id: "One".into(),
            broker_id: "Broker2".into()
        })
        .is_err()
    );
    assert!(j.state().order("Two").unwrap().broker_id.is_none());
}
#[test]
fn mock_rejection_is_terminal_and_does_not_retry() {
    let dir = TestRedis::new();
    let mut j = Journal::create_at(&dir.url, &dir.namespace()).unwrap();
    prepare(&mut j, "Rejected");
    let mut broker = MockBroker::new(Outcome::Rejected);
    coordinator::submit(&mut j, &mut broker, "Rejected", &mut limit(&dir)).unwrap();
    assert_eq!(
        j.state().order("Rejected").unwrap().status,
        Status::Rejected
    );
    assert!(coordinator::submit(&mut j, &mut broker, "Rejected", &mut limit(&dir)).is_err());
    assert_eq!(broker.calls, 1);
}
#[test]
fn translation_preserves_contract_quantity_product_and_exact_price() {
    let mut i = intent("Tag1");
    i.product = Product::Mis;
    i.side = Side::Sell;
    let request = translation::translate(&i).unwrap();
    assert_eq!(request.quantity, 2);
    assert_eq!(request.price_rupees, 6000);
    assert_eq!(request.transaction_type, "SELL");
    assert_eq!(request.product, "MIS");
    assert_eq!(request.tag, "Tag1");
    assert_eq!(request.order_type, "LIMIT");
    i.limit_price_paise = 600001;
    assert!(translation::translate(&i).is_err());
    i.limit_price_paise = 600000;
    i.quantity = 0;
    assert!(translation::translate(&i).is_err());
    i.quantity = 2;
    i.id = "bad-tag".into();
    assert!(translation::translate(&i).is_err());
}
#[test]
fn stale_writer_is_rejected_before_a_second_mock_submission() {
    let dir = TestRedis::new();
    let path = dir.namespace();
    let mut first = Journal::create_at(&dir.url, &path).unwrap();
    prepare(&mut first, "One");
    let mut second = Journal::open_at(&dir.url, &path).unwrap();
    let mut broker = MockBroker::new(Outcome::Accepted("Broker1".into()));
    coordinator::submit(&mut first, &mut broker, "One", &mut limit(&dir)).unwrap();
    assert!(coordinator::submit(&mut second, &mut broker, "One", &mut limit(&dir)).is_err());
    assert_eq!(broker.calls, 1);
}
#[test]
fn buy_fill_above_limit_is_rejected() {
    let dir = TestRedis::new();
    let mut j = Journal::create_at(&dir.url, &dir.namespace()).unwrap();
    prepare(&mut j, "Limit");
    dispatch(&mut j, "Limit");
    let event = Event::Fill {
        id: "Limit".into(),
        broker_id: "Broker1".into(),
        trade_id: "T1".into(),
        quantity: 1,
        price_paise: 600100,
    };
    assert!(j.append(event).is_err());
    assert!(j.state().order("Limit").unwrap().broker_id.is_none());
}
#[test]
#[ignore = "child process fixture"]
fn abrupt_exit_child() {
    let path = std::env::var("KITE_TEST_JOURNAL").unwrap();
    let url = std::env::var("KITE_TEST_REDIS_URL").unwrap();
    let mut j = Journal::create_at(&url, &path).unwrap();
    prepare(&mut j, "Interrupted");
    dispatch(&mut j, "Interrupted");
    std::process::exit(23);
}
#[test]
fn durable_dispatch_survives_process_exit_without_destructors() {
    let dir = TestRedis::new();
    let path = dir.namespace();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "abrupt_exit_child"])
        .env("KITE_TEST_JOURNAL", &path)
        .env("KITE_TEST_REDIS_URL", &dir.url)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(23));
    let mut j = Journal::open_at(&dir.url, &path).unwrap();
    assert_eq!(
        j.state().order("Interrupted").unwrap().status,
        Status::Dispatching
    );
    let mut broker = MockBroker::new(Outcome::Accepted("Never".into()));
    assert!(coordinator::submit(&mut j, &mut broker, "Interrupted", &mut limit(&dir)).is_err());
    assert_eq!(broker.calls, 0);
}

fn limit(dir: &TestRedis) -> kite_execution::rate_limit::Limiter {
    use kite_execution::rate_limit::{Limiter, policy::Policy};
    Limiter::create_at(&dir.url, "execution-tests", Policy::default())
        .or_else(|_| Limiter::open_at(&dir.url, "execution-tests", Policy::default()))
        .unwrap()
}

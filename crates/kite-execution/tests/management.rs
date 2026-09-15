use kite_execution::{
    management::{cancel, coordinator as manage, modify, verification},
    mock::{MockBroker, Outcome},
    rate_limit::{Limiter, policy::Policy},
};
use kite_journal::{
    actions::{Action, Change, CommandStatus},
    model::{Event, Intent, Product, Side},
    state::Status,
    store::Journal,
};
#[path = "../../kite-journal/test-support/redis.rs"]
mod support;
use support::TestRedis;
fn setup(server: &TestRedis) -> Journal {
    let mut j = Journal::create_at(&server.url, "test").unwrap();
    j.append(Event::Intent {
        intent: Intent {
            id: "Order".into(),
            symbol: "CRUDEOIL26SEPFUT".into(),
            side: Side::Buy,
            product: Product::Nrml,
            quantity: 2,
            limit_price_paise: 600000,
        },
    })
    .unwrap();
    j.append(Event::Dispatch { id: "Order".into() }).unwrap();
    j.append(Event::Acknowledged {
        id: "Order".into(),
        broker_id: "Broker1".into(),
    })
    .unwrap();
    j
}
fn action(j: &mut Journal, a: Action) -> anyhow::Result<bool> {
    j.append(Event::Management {
        id: "Order".into(),
        action: a,
    })
}
fn fill(j: &mut Journal, quantity: u32) {
    j.append(Event::Fill {
        id: "Order".into(),
        broker_id: "Broker1".into(),
        trade_id: "Trade1".into(),
        quantity,
        price_paise: 600000,
    })
    .unwrap();
}
#[test]
fn cli_management_scenario_preserves_partial_fill_and_blocks_retry() {
    let server = TestRedis::new();
    let result = verification::run_at(&server.url, "check").unwrap();
    assert_eq!(result.mock_calls, 5);
    assert!(result.ambiguous_cancel_retry_blocked_after_restart);
    assert!(result.modification_confirmed && result.partial_fill_preserved);
}
#[test]
fn acknowledgement_does_not_change_terms_and_confirmation_survives_restart() {
    let server = TestRedis::new();
    let mut j = setup(&server);
    let mut limiter = Limiter::create_at(&server.url, "account", Policy::default()).unwrap();
    manage::prepare(
        &mut j,
        "Order",
        "M1",
        Change::Modify {
            quantity: 3,
            limit_price_paise: 600100,
        },
    )
    .unwrap();
    let mut broker = MockBroker::new(Outcome::Accepted("Broker1".into()));
    manage::send(&mut j, &mut broker, "Order", "M1", &mut limiter).unwrap();
    assert_eq!(j.state().order("Order").unwrap().intent.quantity, 2);
    assert_eq!(
        j.state().order("Order").unwrap().commands.entries["M1"].status,
        CommandStatus::Acknowledged
    );
    manage::confirm(&mut j, "Order", "M1", "Broker1").unwrap();
    assert!(!manage::confirm(&mut j, "Order", "M1", "Broker1").unwrap());
    drop(j);
    let j = Journal::open_at(&server.url, "test").unwrap();
    assert_eq!(
        j.state().order("Order").unwrap().intent.limit_price_paise,
        600100
    );
    assert_eq!(
        j.state()
            .order("Order")
            .unwrap()
            .commands
            .modification_attempts,
        1
    );
}
#[test]
fn process_interruption_after_dispatch_blocks_management_resend() {
    let server = TestRedis::new();
    let mut j = setup(&server);
    manage::prepare(&mut j, "Order", "C1", Change::Cancel).unwrap();
    action(
        &mut j,
        Action::Dispatch {
            command_id: "C1".into(),
        },
    )
    .unwrap();
    drop(j);
    let mut j = Journal::open_at(&server.url, "test").unwrap();
    let mut limiter = Limiter::create_at(&server.url, "account", Policy::default()).unwrap();
    let mut broker = MockBroker::new(Outcome::Accepted("Broker1".into()));
    assert!(manage::send(&mut j, &mut broker, "Order", "C1", &mut limiter).is_err());
    assert_eq!(broker.calls, 0);
    assert!(manage::prepare(&mut j, "Order", "C2", Change::Cancel).is_err());
}
#[test]
fn fills_can_win_cancel_race_without_being_erased() {
    let server = TestRedis::new();
    let mut j = setup(&server);
    manage::prepare(&mut j, "Order", "C1", Change::Cancel).unwrap();
    action(
        &mut j,
        Action::Dispatch {
            command_id: "C1".into(),
        },
    )
    .unwrap();
    fill(&mut j, 2);
    manage::confirm(&mut j, "Order", "C1", "Broker1").unwrap();
    assert_eq!(j.state().order("Order").unwrap().status, Status::Filled);
    assert_eq!(j.state().order("Order").unwrap().filled, 2);
    assert!(manage::prepare(&mut j, "Order", "C2", Change::Cancel).is_err());
}
#[test]
fn identity_mismatch_and_duplicate_command_ids_fail_closed() {
    let server = TestRedis::new();
    let mut j = setup(&server);
    manage::prepare(&mut j, "Order", "C1", Change::Cancel).unwrap();
    assert!(manage::prepare(&mut j, "Order", "C1", Change::Cancel).is_err());
    assert!(manage::confirm(&mut j, "Order", "C1", "Broker1").is_err());
    action(
        &mut j,
        Action::Dispatch {
            command_id: "C1".into(),
        },
    )
    .unwrap();
    assert!(manage::confirm(&mut j, "Order", "C1", "OtherBroker").is_err());
    assert_eq!(j.state().order("Order").unwrap().status, Status::Accepted);
}
#[test]
fn rejected_modification_keeps_order_terms_and_consumes_attempt() {
    let server = TestRedis::new();
    let mut j = setup(&server);
    manage::prepare(
        &mut j,
        "Order",
        "M1",
        Change::Modify {
            quantity: 3,
            limit_price_paise: 600100,
        },
    )
    .unwrap();
    let mut limiter = Limiter::create_at(&server.url, "account", Policy::default()).unwrap();
    let mut broker = MockBroker::new(Outcome::Rejected);
    manage::send(&mut j, &mut broker, "Order", "M1", &mut limiter).unwrap();
    assert_eq!(j.state().order("Order").unwrap().intent.quantity, 2);
    assert_eq!(
        j.state()
            .order("Order")
            .unwrap()
            .commands
            .modification_attempts,
        1
    );
    assert!(manage::send(&mut j, &mut broker, "Order", "M1", &mut limiter).is_err());
    manage::prepare(
        &mut j,
        "Order",
        "M2",
        Change::Modify {
            quantity: 3,
            limit_price_paise: 600100,
        },
    )
    .unwrap();
}
#[test]
fn twenty_sixth_modification_is_blocked_after_restart() {
    let server = TestRedis::new();
    let mut j = setup(&server);
    for n in 1..=25 {
        let id = format!("M{n}");
        let quantity = if n % 2 == 1 { 3 } else { 2 };
        manage::prepare(
            &mut j,
            "Order",
            &id,
            Change::Modify {
                quantity,
                limit_price_paise: 600000,
            },
        )
        .unwrap();
        action(
            &mut j,
            Action::Dispatch {
                command_id: id.clone(),
            },
        )
        .unwrap();
        manage::confirm(&mut j, "Order", &id, "Broker1").unwrap();
    }
    drop(j);
    let mut j = Journal::open_at(&server.url, "test").unwrap();
    assert!(
        manage::prepare(
            &mut j,
            "Order",
            "M26",
            Change::Modify {
                quantity: 4,
                limit_price_paise: 600000
            }
        )
        .is_err()
    );
    assert_eq!(
        j.state()
            .order("Order")
            .unwrap()
            .commands
            .modification_attempts,
        25
    );
}
#[test]
fn translations_and_quantity_checks_preserve_contract_units() {
    let server = TestRedis::new();
    let mut j = setup(&server);
    fill(&mut j, 1);
    let o = j.state().order("Order").unwrap();
    let m = modify::translate(o, 3, 600100).unwrap();
    assert_eq!(m.quantity, 3);
    assert_eq!(m.price_rupees, 6001);
    assert_eq!(m.broker_id, "Broker1");
    assert_eq!(cancel::translate(o).unwrap().variety, "regular");
    assert!(modify::translate(o, 1, 600100).is_err());
    assert!(modify::translate(o, 3, 600001).is_err());
    assert!(modify::translate(o, 2, 600000).is_err());
}
#[test]
fn budget_deferral_leaves_command_prepared_without_attempt_count() {
    let server = TestRedis::new();
    let mut j = setup(&server);
    let mut limiter = Limiter::create_at(
        &server.url,
        "account",
        Policy {
            per_second: 1,
            per_minute: 1,
            per_day: 1,
        },
    )
    .unwrap();
    limiter.reserve().unwrap();
    manage::prepare(
        &mut j,
        "Order",
        "M1",
        Change::Modify {
            quantity: 3,
            limit_price_paise: 600100,
        },
    )
    .unwrap();
    let mut broker = MockBroker::new(Outcome::Accepted("Broker1".into()));
    assert!(manage::send(&mut j, &mut broker, "Order", "M1", &mut limiter).is_err());
    assert_eq!(broker.calls, 0);
    assert_eq!(
        j.state().order("Order").unwrap().commands.entries["M1"].status,
        CommandStatus::Prepared
    );
    assert_eq!(
        j.state()
            .order("Order")
            .unwrap()
            .commands
            .modification_attempts,
        0
    );
}

use kite_execution::rate_limit::{Decision, Limiter, policy::Policy, verification};
#[path = "../../kite-journal/test-support/redis.rs"]
mod support;
use std::sync::{Arc, Barrier};
use support::TestRedis;
#[test]
fn concurrent_workers_share_one_account_budget() {
    let server = TestRedis::new();
    let p = Policy {
        per_second: 10,
        per_minute: 400,
        per_day: 3,
    };
    let _limiter = Limiter::create_at(&server.url, "account", p).unwrap();
    let barrier = Arc::new(Barrier::new(12));
    let workers = (0..12)
        .map(|_| {
            let url = server.url.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut limiter = Limiter::open_at(&url, "account", p).unwrap();
                barrier.wait();
                matches!(limiter.reserve().unwrap(), Decision::Allowed) as usize
            })
        })
        .collect::<Vec<_>>();
    let admitted: usize = workers.into_iter().map(|w| w.join().unwrap()).sum();
    assert_eq!(admitted, 3);
}
#[test]
fn cooldown_is_shared_and_cannot_be_shortened() {
    let server = TestRedis::new();
    let mut first = Limiter::create_at(&server.url, "account", Policy::default()).unwrap();
    let mut second = Limiter::open_at(&server.url, "account", Policy::default()).unwrap();
    first.cooldown(60000).unwrap();
    second.cooldown(1).unwrap();
    assert!(
        matches!(second.reserve().unwrap(),Decision::Deferred {retry_after_ms} if retry_after_ms>59000)
    );
}
#[test]
fn redis_restart_preserves_budget_and_cooldown() {
    let mut server = TestRedis::new();
    let p = Policy {
        per_second: 10,
        per_minute: 400,
        per_day: 1,
    };
    let mut limiter = Limiter::create_at(&server.url, "account", p).unwrap();
    assert_eq!(limiter.reserve().unwrap(), Decision::Allowed);
    limiter.cooldown(60000).unwrap();
    drop(limiter);
    server.restart();
    let mut limiter = Limiter::open_at(&server.url, "account", p).unwrap();
    assert!(
        matches!(limiter.reserve().unwrap(),Decision::Deferred {retry_after_ms} if retry_after_ms>86000000)
    );
}
fn history(server: &TestRedis, scope: &str, age: u64) {
    let mut c = server.connection();
    let key = Limiter::key(scope).unwrap();
    let (seconds, micros): (u64, u64) = redis::cmd("TIME").query(&mut c).unwrap();
    let now = seconds * 1000 + micros / 1000;
    let raw: String = redis::cmd("GET").arg(&key).query(&mut c).unwrap();
    let mut state: serde_json::Value = serde_json::from_str(&raw).unwrap();
    state["history"] = serde_json::json!([now - age]);
    state["last_ms"] = serde_json::json!(now - age);
    redis::cmd("SET")
        .arg(&key)
        .arg(state.to_string())
        .query::<()>(&mut c)
        .unwrap();
}
#[test]
fn rolling_second_minute_and_day_limits_are_independent() {
    let server = TestRedis::new();
    let mut second = Limiter::create_at(
        &server.url,
        "second",
        Policy {
            per_second: 1,
            per_minute: 400,
            per_day: 5000,
        },
    )
    .unwrap();
    history(&server, "second", 0);
    assert!(matches!(
        second.reserve().unwrap(),
        Decision::Deferred { .. }
    ));
    history(&server, "second", 2000);
    assert_eq!(second.reserve().unwrap(), Decision::Allowed);
    let mut minute = Limiter::create_at(
        &server.url,
        "minute",
        Policy {
            per_second: 10,
            per_minute: 1,
            per_day: 5000,
        },
    )
    .unwrap();
    history(&server, "minute", 2000);
    assert!(matches!(
        minute.reserve().unwrap(),
        Decision::Deferred { .. }
    ));
    history(&server, "minute", 61000);
    assert_eq!(minute.reserve().unwrap(), Decision::Allowed);
    let mut day = Limiter::create_at(
        &server.url,
        "day",
        Policy {
            per_second: 10,
            per_minute: 400,
            per_day: 1,
        },
    )
    .unwrap();
    history(&server, "day", 61000);
    assert!(matches!(day.reserve().unwrap(), Decision::Deferred { .. }));
    history(&server, "day", 86400001);
    assert_eq!(day.reserve().unwrap(), Decision::Allowed);
}
#[test]
fn account_isolation_policy_mismatch_and_missing_state() {
    let server = TestRedis::new();
    let p = Policy {
        per_second: 1,
        per_minute: 1,
        per_day: 1,
    };
    let mut first = Limiter::create_at(&server.url, "a", p).unwrap();
    assert_eq!(first.reserve().unwrap(), Decision::Allowed);
    let mut second = Limiter::create_at(&server.url, "b", p).unwrap();
    assert_eq!(second.reserve().unwrap(), Decision::Allowed);
    assert!(Limiter::open_at(&server.url, "a", Policy::default()).is_err());
    assert!(Limiter::open_at(&server.url, "missing", p).is_err());
    assert!(Limiter::create_at(&server.url, "a", p).is_err());
}
#[test]
fn corrupt_state_and_backward_clock_block_requests() {
    let server = TestRedis::new();
    let mut limiter = Limiter::create_at(&server.url, "a", Policy::default()).unwrap();
    let mut c = server.connection();
    let key = Limiter::key("a").unwrap();
    let raw: String = redis::cmd("GET").arg(&key).query(&mut c).unwrap();
    let mut state: serde_json::Value = serde_json::from_str(&raw).unwrap();
    state["last_ms"] = serde_json::json!(9000000000000_u64);
    redis::cmd("SET")
        .arg(&key)
        .arg(state.to_string())
        .query::<()>(&mut c)
        .unwrap();
    assert!(limiter.reserve().is_err());
    redis::cmd("SET")
        .arg(&key)
        .arg(&raw)
        .query::<()>(&mut c)
        .unwrap();
    assert!(limiter.reserve().is_err()); // poisoned handle never resumes blindly
    let mut limiter = Limiter::open_at(&server.url, "a", Policy::default()).unwrap();
    redis::cmd("SET")
        .arg(&key)
        .arg("private-sentinel")
        .query::<()>(&mut c)
        .unwrap();
    assert!(
        !limiter
            .reserve()
            .unwrap_err()
            .to_string()
            .contains("private-sentinel")
    );
}
#[test]
fn cli_scenario_gates_mock_submission() {
    let server = TestRedis::new();
    let summary = verification::run_at(&server.url, "check").unwrap();
    assert_eq!(summary.admitted, 1);
    assert_eq!(summary.blocked_mock_calls, 0);
    assert!(summary.deferred_intent_stays_prepared);
}
#[test]
fn mock_429_updates_shared_cooldown() {
    use kite_execution::{
        coordinator,
        mock::{MockBroker, Outcome},
    };
    use kite_journal::{
        model::{Event, Intent, Product, Side},
        state::Status,
        store::Journal,
    };
    let server = TestRedis::new();
    let mut limiter = Limiter::create_at(&server.url, "account", Policy::default()).unwrap();
    let mut journal = Journal::create_at(&server.url, "test").unwrap();
    for id in ["One", "Two"] {
        journal
            .append(Event::Intent {
                intent: Intent {
                    id: id.into(),
                    symbol: "CRUDEOIL26SEPFUT".into(),
                    side: Side::Buy,
                    product: Product::Nrml,
                    quantity: 1,
                    limit_price_paise: 600000,
                },
            })
            .unwrap();
    }
    let mut broker = MockBroker::new(Outcome::RateLimited {
        retry_after_ms: 60000,
    });
    coordinator::submit(&mut journal, &mut broker, "One", &mut limiter).unwrap();
    assert_eq!(
        journal.state().order("One").unwrap().status,
        Status::Unknown
    );
    let mut other = Limiter::open_at(&server.url, "account", Policy::default()).unwrap();
    assert!(coordinator::submit(&mut journal, &mut broker, "Two", &mut other).is_err());
    assert_eq!(broker.calls, 1);
    assert_eq!(
        journal.state().order("Two").unwrap().status,
        Status::Prepared
    );
}

use super::coordination::{Account, key, status_at};
#[path = "../../../../kite-journal/test-support/redis.rs"]
mod support;
use support::TestRedis;
#[test]
fn only_one_account_owner_across_threads_and_no_crash_takeover() {
    let server = TestRedis::new();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let threads = (0..4)
        .map(|i| {
            let b = barrier.clone();
            let url = server.url.clone();
            std::thread::spawn(move || {
                b.wait();
                Account::acquire(&url, "MOCK", &format!("worker-{i}")).is_ok() as usize
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .sum::<usize>(),
        1
    );
    assert!(Account::acquire(&server.url, "MOCK", "restart").is_err());
    assert!(Account::acquire(&server.url, "OTHER", "other").is_ok());
}
#[test]
fn clean_restart_preserves_budget_and_cooldown_and_missing_budget_blocks() {
    let server = TestRedis::new();
    let mut first = Account::acquire(&server.url, "MOCK", "one").unwrap();
    first.reserve().unwrap();
    first.cooldown(60000).unwrap();
    first.finish(true, 0, 0).unwrap();
    let mut second = Account::acquire(&server.url, "MOCK", "two").unwrap();
    assert!(second.reserve().is_err());
    second.finish(true, 0, 0).unwrap();
    let mut c = server.connection();
    redis::cmd("DEL")
        .arg(kite_execution::rate_limit::Limiter::key("native-account-MOCK").unwrap())
        .query::<()>(&mut c)
        .unwrap();
    assert!(Account::acquire(&server.url, "MOCK", "three").is_err());
}
#[test]
fn redis_restart_retains_owner_and_stale_owner_cannot_release_or_dispatch() {
    let mut server = TestRedis::new();
    let owner = Account::acquire(&server.url, "MOCK", "one").unwrap();
    drop(owner);
    server.restart();
    assert!(Account::acquire(&server.url, "MOCK", "two").is_err());
    let mut other = Account::acquire(&server.url, "OTHER", "other").unwrap();
    redis::cmd("HSET")
        .arg(key("OTHER").unwrap())
        .arg("owner")
        .arg("replacement")
        .query::<()>(&mut server.connection())
        .unwrap();
    assert!(other.reserve().is_err());
    assert!(other.finish(true, 0, 0).is_err());
}
#[test]
fn unresolved_shutdown_is_durable_and_cannot_be_cleared_by_heartbeat() {
    let server = TestRedis::new();
    let mut a = Account::acquire(&server.url, "MOCK", "one").unwrap();
    a.finish(false, 1, -1).unwrap();
    let s = status_at(&server.url, "MOCK").unwrap();
    assert_eq!(s["state"], "ReviewRequired");
    assert_eq!(s["position"], "-1");
    assert_eq!(s["requires_review"], true);
    assert!(a.update("Running", 0, 0).is_err());
    assert!(a.finish(true, 0, 0).is_err());
    assert!(Account::acquire(&server.url, "MOCK", "two").is_err());
}

#[test]
fn sandbox_credentials_never_fall_back_to_production_keys() {
    let server = TestRedis::new();
    let mut c = server.connection();
    redis::cmd("MSET")
        .arg("susanta:kite_api_key")
        .arg("production-sentinel")
        .arg("susanta:kite_access_token")
        .arg("production-token")
        .query::<()>(&mut c)
        .unwrap();
    assert!(crate::credentials::redis::load_sandbox_at(&server.url).is_err());
    redis::cmd("MSET")
        .arg("sandbox:kite_api_key")
        .arg("sandbox-only")
        .arg("sandbox:kite_access_token")
        .arg("sandbox-token")
        .query::<()>(&mut c)
        .unwrap();
    let credentials = crate::credentials::redis::load_sandbox_at(&server.url).unwrap();
    assert_eq!(credentials.api_key(), "sandbox-only");
    assert_eq!(credentials.access_token(), "sandbox-token");
}

use kite_journal::{
    model::{Event, Intent, Product, Side},
    store::Journal,
};
#[path = "../test-support/redis.rs"]
mod support;
use support::TestRedis;
fn event() -> Event {
    Event::Intent {
        intent: Intent {
            id: "Test".into(),
            symbol: "CRUDEOIL26SEPFUT".into(),
            side: Side::Buy,
            product: Product::Nrml,
            quantity: 1,
            limit_price_paise: 600000,
        },
    }
}
#[test]
fn write_failure_poisoning_blocks_further_actions() {
    let server = TestRedis::new();
    let mut j = Journal::create_at(&server.url, "test").unwrap();
    let key = Journal::key("test").unwrap();
    let mut c = server.connection();
    redis::cmd("DEL").arg(&key).query::<()>(&mut c).unwrap();
    assert!(j.append(event()).is_err());
    let _replacement = Journal::create_at(&server.url, "test").unwrap();
    assert!(j.append(event()).is_err());
    assert_eq!(j.record_count(), 0);
}
#[test]
fn corrupt_payload_and_scope_fail_closed() {
    let server = TestRedis::new();
    let mut j = Journal::create_at(&server.url, "test").unwrap();
    j.append(event()).unwrap();
    drop(j);
    let key = Journal::key("test").unwrap();
    let mut c = server.connection();
    redis::cmd("HSET")
        .arg(&key)
        .arg("event:1")
        .arg("private-sentinel")
        .query::<()>(&mut c)
        .unwrap();
    let error = Journal::open_at(&server.url, "test")
        .err()
        .unwrap()
        .to_string();
    assert!(!error.contains("private-sentinel"));
    redis::cmd("HSET")
        .arg(&key)
        .arg("scope")
        .arg("OTHER")
        .query::<()>(&mut c)
        .unwrap();
    assert!(Journal::open_at(&server.url, "test").is_err());
}
#[test]
fn missing_namespace_sequence_gap_and_ttl_fail_closed() {
    let server = TestRedis::new();
    assert!(Journal::open_at(&server.url, "test").is_err());
    let mut j = Journal::create_at(&server.url, "test").unwrap();
    j.append(event()).unwrap();
    drop(j);
    let key = Journal::key("test").unwrap();
    let mut c = server.connection();
    redis::cmd("HDEL")
        .arg(&key)
        .arg("event:1")
        .query::<()>(&mut c)
        .unwrap();
    assert!(Journal::open_at(&server.url, "test").is_err());
    let mut j = Journal::create_at(&server.url, "ttl").unwrap();
    redis::cmd("EXPIRE")
        .arg(Journal::key("ttl").unwrap())
        .arg(60)
        .query::<()>(&mut c)
        .unwrap();
    assert!(j.append(event()).is_err());
}
#[test]
fn replaced_namespace_cannot_be_written_by_old_generation() {
    let server = TestRedis::new();
    let mut old = Journal::create_at(&server.url, "test").unwrap();
    let mut c = server.connection();
    redis::cmd("DEL")
        .arg(Journal::key("test").unwrap())
        .query::<()>(&mut c)
        .unwrap();
    let replacement = Journal::create_at(&server.url, "test").unwrap();
    assert!(old.append(event()).is_err());
    assert_eq!(replacement.record_count(), 0);
}
#[test]
fn aof_disabled_is_rejected_before_creating_journal() {
    let server = TestRedis::new();
    let mut c = server.connection();
    redis::cmd("CONFIG")
        .arg("SET")
        .arg("appendonly")
        .arg("no")
        .query::<()>(&mut c)
        .unwrap();
    assert!(Journal::create_at(&server.url, "test").is_err());
    assert_eq!(
        redis::cmd("EXISTS")
            .arg(Journal::key("test").unwrap())
            .query::<u32>(&mut c)
            .unwrap(),
        0
    );
}
#[test]
fn aof_recovers_after_redis_process_is_killed() {
    let mut server = TestRedis::new();
    let mut j = Journal::create_at(&server.url, "test").unwrap();
    j.append(event()).unwrap();
    j.append(Event::Dispatch { id: "Test".into() }).unwrap();
    drop(j);
    server.restart();
    let j = Journal::open_at(&server.url, "test").unwrap();
    assert_eq!(j.record_count(), 2);
    assert_eq!(
        j.state().order("Test").unwrap().status,
        kite_journal::state::Status::Dispatching
    );
}

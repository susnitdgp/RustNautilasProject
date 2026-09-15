use super::{
    recovery,
    risk::Controls,
    session::{Session, with_actor},
    simulation::{fixture, quote},
};
use crate::native_paper_command::redis_support as support;
use kite_strategy::config::Config;
use nautilus_model::enums::OrderSide;
fn config() -> Config {
    Config::parse(include_str!("../../../../config/strategy-crossover.toml")).unwrap()
}
fn warmup(session: &mut Session, count: usize) {
    let (instrument, ts) = fixture().unwrap();
    for (i, p) in [
        6008, 6006, 6004, 6002, 6000, 6002, 6004, 6006, 6005, 6005, 6003, 6003, 6001, 5999, 6000,
    ]
    .iter()
    .take(count)
    .enumerate()
    {
        let now = ts.as_u64() + (i as u64 + 1) * 1_000_000_000;
        session.quote(quote(&instrument, *p, now), 1, now).unwrap();
    }
}
#[test]
fn native_actor_risk_execution_roundtrip() {
    let redis = support::TestRedis::new();
    let result = super::simulation::run_at(&redis.url, "flow", config()).unwrap();
    assert_eq!(result["quotes"], 15);
    assert_eq!(result["full_ticks"], 15);
    assert_eq!(result["data_mode"], "full");
    assert_eq!(result["signals"], 2);
    assert_eq!(result["paper_fills"], 2);
    assert_eq!(result["denied"], 0);
    let result = recovery::inspect(&redis.url, "flow").unwrap();
    assert_eq!(result["native_replay_verified"], true);
    assert_eq!(result["requires_review"], false);
    assert_eq!(result["resubmissions"], 0);
}
#[test]
fn disconnect_cancels_pending_and_reconnect_requires_warmup() {
    let redis = support::TestRedis::new();
    let (instrument, ts) = fixture().unwrap();
    let mut session = Session::new(&redis.url, "gap", config(), &instrument, ts).unwrap();
    session.connected(1).unwrap();
    warmup(&mut session, 8);
    assert!(with_actor(|a| a.pending.is_some()));
    session.gap().unwrap();
    assert!(with_actor(|a| a.pending.is_none()));
    assert_eq!(with_actor(|a| a.cancels), 1);
    let now = ts.as_u64() + 20_000_000_000;
    session
        .quote(quote(&instrument, 5000, now), 1, now)
        .unwrap();
    assert_eq!(with_actor(|a| a.fills), 0);
    session.connected(2).unwrap();
    session
        .quote(quote(&instrument, 5000, now + 1), 2, now + 1)
        .unwrap();
    assert_eq!(with_actor(|a| a.logic.mids.len()), 1);
    assert_eq!(with_actor(|a| a.signals), 1);
    session.finish().unwrap();
}
#[test]
fn stale_quote_cancels_without_filling_and_recovers_on_fresh_data() {
    let redis = support::TestRedis::new();
    let (instrument, ts) = fixture().unwrap();
    let mut session = Session::new(&redis.url, "stale-flow", config(), &instrument, ts).unwrap();
    session.connected(1).unwrap();
    warmup(&mut session, 8);
    let q = quote(&instrument, 5000, ts.as_u64() + 9_000_000_000);
    session.quote(q, 1, ts.as_u64() + 40_000_000_000).unwrap();
    assert_eq!(with_actor(|a| a.fills), 0);
    assert_eq!(with_actor(|a| a.cancels), 1);
    let now = ts.as_u64() + 41_000_000_000;
    session
        .quote(quote(&instrument, 5000, now), 1, now)
        .unwrap();
    assert_eq!(with_actor(|a| a.logic.mids.len()), 1);
    session.finish().unwrap();
}
#[test]
fn native_risk_denial_is_persisted_and_replayable() {
    let redis = support::TestRedis::new();
    let (instrument, ts) = fixture().unwrap();
    let mut session = Session::new(&redis.url, "denial", config(), &instrument, ts).unwrap();
    session.native_notional_limit(instrument.id, rust_decimal::Decimal::from(1));
    session.connected(1).unwrap();
    warmup(&mut session, 8);
    assert_eq!(with_actor(|a| a.denied), 1);
    assert_eq!(with_actor(|a| a.fills), 0);
    session.finish().unwrap();
    drop(session);
    assert_eq!(
        kite_paper::outbox::Outbox::read_at(&redis.url, "denial")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        recovery::inspect(&redis.url, "denial").unwrap()["requires_review"],
        false
    );
}
#[test]
fn additional_risk_guards_reject_wrong_position_and_excess_notional() {
    let (instrument, ts) = fixture().unwrap();
    let mut controls = Controls::default();
    controls.connect(1).unwrap();
    controls
        .quote(
            &quote(&instrument, 6000, ts.as_u64()),
            1,
            ts.as_u64(),
            &config(),
        )
        .unwrap();
    let o = crate::native_paper_command::order(
        "R1",
        OrderSide::Buy,
        nautilus_model::types::Price::new(6000.0, 0),
        ts,
    );
    assert!(
        controls
            .order(o.init_event(), 1, ts.as_u64(), &config())
            .is_err()
    );
    controls.max_notional = rust_decimal::Decimal::from(100);
    assert!(
        controls
            .order(o.init_event(), 0, ts.as_u64(), &config())
            .is_err()
    );
    controls.gap();
    assert!(
        controls
            .order(o.init_event(), 0, ts.as_u64(), &config())
            .is_err()
    );
}
#[test]
fn crash_child() {
    let Ok(url) = std::env::var("KITE_FLOW_TEST_REDIS") else {
        return;
    };
    let (instrument, ts) = fixture().unwrap();
    let mut session = Session::new(&url, "crashed-flow", config(), &instrument, ts).unwrap();
    session.connected(1).unwrap();
    warmup(&mut session, 8);
    assert!(with_actor(|a| a.pending.is_some()));
    std::process::exit(17);
}
#[test]
fn abrupt_process_exit_and_redis_restart_do_not_resubmit() {
    let mut redis = support::TestRedis::new();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "paper_flow::tests::crash_child", "--nocapture"])
        .env("KITE_FLOW_TEST_REDIS", &redis.url)
        .output()
        .unwrap();
    assert_eq!(
        child.status.code(),
        Some(17),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    redis.restart();
    let result = recovery::inspect(&redis.url, "crashed-flow").unwrap();
    assert_eq!(result["session_finished"], false);
    assert_eq!(result["unresolved_orders"], 1);
    assert_eq!(result["resubmissions"], 0);
    assert_eq!(result["requires_review"], true);
    let (instrument, ts) = fixture().unwrap();
    assert!(Session::new(&redis.url, "crashed-flow", config(), &instrument, ts).is_err());
}

#[test]
fn quotes_buffered_before_acceptance_cannot_fill() {
    let redis = support::TestRedis::new();
    let (instrument, ts) = fixture().unwrap();
    let mut session = Session::new(&redis.url, "buffered", config(), &instrument, ts).unwrap();
    session.connected(1).unwrap();
    warmup(&mut session, 8);
    session.match_after_ns = ts.as_u64() + 10_000_000_000;
    let earlier = ts.as_u64() + 9_000_000_000;
    session
        .quote(quote(&instrument, 5000, earlier), 1, earlier)
        .unwrap();
    assert_eq!(with_actor(|a| a.fills), 0);
    assert!(with_actor(|a| a.pending.is_some()));
    let later = ts.as_u64() + 11_000_000_000;
    session
        .quote(quote(&instrument, 5000, later), 1, later)
        .unwrap();
    assert_eq!(with_actor(|a| a.fills), 1);
    session.finish().unwrap();
}

#[test]
fn full_ticks_retain_depth_and_report_precise_rejection_reason() {
    let redis = support::TestRedis::new();
    let (instrument, ts) = fixture().unwrap();
    let mut session = Session::new(&redis.url, "full-depth", config(), &instrument, ts).unwrap();
    session.expected_token = Some(144870151);
    session.connected(1).unwrap();
    let full = super::simulation::full_snapshot(6000, ts.as_u64() + 1_000_000_000, 1);
    session.full(full, ts.as_u64() + 1_000_000_000).unwrap();
    assert_eq!(with_actor(|a| a.full_ticks), 1);
    assert_eq!(
        with_actor(|a| a
            .last_full_tick
            .as_ref()
            .unwrap()
            .snapshot
            .raw
            .as_ref()
            .unwrap()
            .full
            .as_ref()
            .unwrap()
            .bids[4]
            .price_paise),
        599600
    );
    let mut wide = super::simulation::full_snapshot(6000, ts.as_u64() + 2_000_000_000, 1);
    wide.ask = Some("6010".into());
    session.full(wide, ts.as_u64() + 2_000_000_000).unwrap();
    assert_eq!(session.diagnostics.rejected["spread_exceeded"], 1);
    assert_eq!(session.diagnostics.socket_gaps, 0);
    assert_eq!(session.diagnostics.quality_suspensions, 1);
    assert_eq!(with_actor(|a| a.full_ticks), 1);
    session.finish().unwrap();
}
#[test]
fn diagnostics_distinguish_queue_delay_from_stale_source() {
    let (instrument, ts) = fixture().unwrap();
    let q = quote(&instrument, 6000, ts.as_u64());
    assert_eq!(
        super::diagnostics::quote_reason(&q, &config(), 0, 0, ts.as_u64() + 11_000_000_000),
        Some("stale_in_queue")
    );
    let mut stale = q;
    stale.ts_init = (ts.as_u64() + 11_000_000_000).into();
    assert_eq!(
        super::diagnostics::quote_reason(&stale, &config(), 0, 0, stale.ts_init.as_u64()),
        Some("stale_at_receipt")
    );
    assert_eq!(
        super::diagnostics::quote_reason(&q, &config(), 0, q.ts_init.as_u64(), ts.as_u64()),
        Some("nonincreasing_receive_timestamp")
    );
}

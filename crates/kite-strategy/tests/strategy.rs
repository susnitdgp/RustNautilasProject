use kite_recorder::records::Record;
use kite_strategy::{
    config::Config,
    crossover::{Signal, Strategy},
    paper,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::QuoteTick,
    identifiers::InstrumentId,
    types::{Price, Quantity},
};
#[path = "../../kite-journal/test-support/redis.rs"]
mod support;
use support::TestRedis;
fn config() -> Config {
    Config {
        fast: 3,
        slow: 5,
        max_spread_rupees: 2,
        max_age_seconds: 10,
        max_entries: 20,
        enable_short: false,
        stop_loss_rupees: 30,
        target_rupees: 60,
    }
}
fn quote(index: u64, bid: i64) -> QuoteTick {
    QuoteTick::new(
        InstrumentId::from("CRUDEOIL26SEPFUT.MCX"),
        Price::new(bid as f64, 0),
        Price::new((bid + 1) as f64, 0),
        Quantity::from(10),
        Quantity::from(10),
        UnixNanos::from(1_000_000_000 * (index + 1)),
        UnixNanos::from(1_000_000_000 * (index + 1)),
    )
}
#[test]
fn warmup_crossover_and_pending_guard() {
    let mut s = Strategy::new(config()).unwrap();
    for (i, p) in [6008, 6006, 6004, 6002, 6000, 6002, 6004]
        .iter()
        .enumerate()
    {
        assert_eq!(s.on_quote(&quote(i as u64, *p)).unwrap(), None);
    }
    assert_eq!(s.on_quote(&quote(7, 6006)).unwrap(), Some(Signal::Buy));
    assert_eq!(s.on_quote(&quote(8, 6005)).unwrap(), None);
    s.filled(Signal::Buy).unwrap();
    assert_eq!(s.position, 1);
    assert_eq!(s.on_quote(&quote(9, 6005)).unwrap(), None);
    assert_eq!(s.on_quote(&quote(10, 6003)).unwrap(), Some(Signal::Sell));
    s.filled(Signal::Sell).unwrap();
    assert_eq!(s.position, 0);
}
#[test]
fn stale_wide_and_out_of_order_quotes_reset_warmup() {
    let mut s = Strategy::new(config()).unwrap();
    s.on_quote(&quote(10, 6000)).unwrap();
    s.on_quote(&quote(9, 6000)).unwrap();
    assert!(s.mids.is_empty());
    let mut wide = quote(11, 6000);
    wide.ask_price = Price::new(6010.0, 0);
    s.on_quote(&wide).unwrap();
    assert!(s.mids.is_empty());
    let mut stale = quote(12, 6000);
    stale.ts_init = UnixNanos::from(100_000_000_000);
    s.on_quote(&stale).unwrap();
    assert!(s.mids.is_empty());
}
#[test]
fn gaps_position_cap_and_invalid_config() {
    let mut s = Strategy::new(config()).unwrap();
    s.on_quote(&quote(0, 6000)).unwrap();
    s.gap();
    assert!(s.mids.is_empty());
    assert!(s.filled(Signal::Sell).is_err());
    let mut c = config();
    c.fast = 5;
    assert!(Strategy::new(c).is_err());
}
#[test]
fn paper_execution_fills_on_following_quote_and_reconciles_redis() {
    let server = TestRedis::new();
    let records = [
        6008, 6006, 6004, 6002, 6000, 6002, 6004, 6006, 6005, 6005, 6003, 6003, 6001, 5999, 6000,
    ]
    .iter()
    .enumerate()
    .map(|(i, p)| Record::Quote {
        quote: quote(i as u64, *p),
        generation: 1,
    })
    .collect();
    let result = paper::run_at(&server.url, config(), "check", records).unwrap();
    assert_eq!(result.signals, 2);
    assert_eq!(result.paper_fills, 2);
    assert_eq!(result.open_contracts, 0);
    assert!(result.checkpoint_verified);
    assert!(!result.broker_accessed && !result.live_orders_enabled);
}
#[test]
fn an_end_of_input_signal_is_cancelled_without_fabricated_fill() {
    let server = TestRedis::new();
    let records = [6008, 6006, 6004, 6002, 6000, 6002, 6004, 6006]
        .iter()
        .enumerate()
        .map(|(i, p)| Record::Quote {
            quote: quote(i as u64, *p),
            generation: 1,
        })
        .collect();
    let result = paper::run_at(&server.url, config(), "check", records).unwrap();
    assert_eq!(result.signals, 1);
    assert_eq!(result.paper_fills, 0);
    assert_eq!(result.cancelled, 1);
}

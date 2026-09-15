use chrono::{NaiveDate, TimeZone, Utc};
use kite_adapter::{
    config::Config,
    instruments::contract,
    mapping::{market_data::Snapshot, quotes},
    preflight,
};
use nautilus_model::instruments::FuturesContract;

fn instrument() -> FuturesContract {
    let config = Config::parse(include_str!("../../../config/crudeoil-september.toml")).unwrap();
    let csv = "instrument_token,tradingsymbol,name,expiry,tick_size,lot_size,instrument_type,segment,exchange\n144870151,CRUDEOIL26SEPFUT,CRUDEOIL,2026-09-21,1,1,FUT,MCX-FUT,MCX\n";
    let report = preflight::run(
        &config,
        csv.as_bytes(),
        NaiveDate::from_ymd_opt(2026, 9, 15).unwrap(),
    )
    .unwrap();
    contract::build(&report, 1u64.into()).unwrap()
}
fn snapshot() -> Snapshot {
    Snapshot {
        instrument_token: 144870151,
        received_at_utc: Utc.timestamp_opt(1789456401, 123).unwrap(),
        connection_generation: 1,
        ltp: "9917.00".into(),
        bid: Some("9916.00".into()),
        ask: Some("9917.00".into()),
        bid_size: Some(3),
        ask_size: Some(4),
        cumulative_volume: Some(5904),
        open_interest: Some(15705),
        exchange_timestamp: Some(1789456400),
        source_fresh: true,
    }
}
#[test]
fn contract_distinguishes_contracts_from_barrels() {
    let instrument = instrument();
    assert_eq!(instrument.multiplier.to_string(), "100");
    assert_eq!(instrument.lot_size.to_string(), "1");
    assert_eq!(instrument.size_increment.to_string(), "1");
    assert_eq!(instrument.price_increment.to_string(), "1");
    assert_eq!(instrument.currency.to_string(), "INR");
    assert!(instrument.activation_ns < instrument.expiration_ns);
    assert_eq!(instrument.expiration_ns.as_u64(), 1790013600000000000);
}
#[test]
fn quote_keeps_contract_sizes_and_both_timestamps() {
    let quote = quotes::map(&snapshot(), &instrument()).unwrap().unwrap();
    assert_eq!(quote.bid_price.to_string(), "9916");
    assert_eq!(quote.ask_price.to_string(), "9917");
    assert_eq!(quote.bid_size.to_string(), "3");
    assert_eq!(quote.ask_size.to_string(), "4");
    assert_eq!(quote.ts_event.as_u64(), 1789456400000000000);
    assert_eq!(quote.ts_init.as_u64(), 1789456401000000123);
}
#[test]
fn missing_or_stale_observations_do_not_become_quotes() {
    let instrument = instrument();
    let mut value = snapshot();
    value.source_fresh = false;
    assert!(quotes::map(&value, &instrument).unwrap().is_none());
    let mut value = snapshot();
    value.bid_size = Some(0);
    assert!(quotes::map(&value, &instrument).unwrap().is_none());
    let mut value = snapshot();
    value.ask = None;
    assert!(quotes::map(&value, &instrument).unwrap().is_none());
    let mut value = snapshot();
    value.exchange_timestamp = None;
    assert!(quotes::map(&value, &instrument).unwrap().is_none());
}
#[test]
fn crossed_and_off_tick_quotes_fail() {
    let instrument = instrument();
    let mut value = snapshot();
    value.bid = Some("9918".into());
    assert!(quotes::map(&value, &instrument).is_err());
    let mut value = snapshot();
    value.bid = Some("9916.25".into());
    assert!(quotes::map(&value, &instrument).is_err());
}

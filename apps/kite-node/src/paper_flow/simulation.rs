use super::session::Session;
use anyhow::Result;
use kite_strategy::config::Config;
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::instruments::FuturesContract;
#[cfg(test)]
use nautilus_model::{
    data::QuoteTick,
    types::{Price, Quantity},
};
pub fn fixture() -> Result<(FuturesContract, UnixNanos)> {
    let ts = UnixNanos::from(1_789_450_000_000_000_000_u64);
    let report = kite_adapter::preflight::Report {
        mode: "data_only",
        instrument_id: "CRUDEOIL26SEPFUT.MCX".into(),
        instrument_token: 144870151,
        expiry: "2026-09-21".into(),
        tick_size: "1".into(),
        broker_lot_size: 1,
        validation_date_ist: chrono::NaiveDate::from_ymd_opt(2026, 9, 15).unwrap(),
        live_orders_enabled: false,
        contract_multiplier_verified: false,
        engine_started: false,
    };
    Ok((kite_adapter::instruments::contract::build(&report, ts)?, ts))
}
/// Synthetic live-clock runs must not expire with the historical replay fixture.
/// Never use this instrument for broker-backed market data or execution.
pub fn live_clock_fixture() -> Result<(FuturesContract, UnixNanos)> {
    let (mut instrument, _) = fixture()?;
    let now = nautilus_core::time::get_atomic_clock_realtime().get_time_ns();
    instrument.expiration_ns = (now.as_u64() + 86_400_000_000_000).into();
    Ok((instrument, now))
}
#[cfg(test)]
pub fn quote(instrument: &FuturesContract, p: i32, ts: u64) -> QuoteTick {
    QuoteTick::new(
        instrument.id,
        Price::new(f64::from(p), 0),
        Price::new(f64::from(p + 1), 0),
        Quantity::from(10),
        Quantity::from(10),
        ts.into(),
        ts.into(),
    )
}
pub fn run(path: &str) -> Result<()> {
    let config = Config::parse(&std::fs::read_to_string(path)?)?;
    let namespace = UUID4::new().to_string();
    let url = kite_journal::connection::url_from_env()?;
    let result = run_at(&url, &namespace, config)?;
    println!(
        "{}",
        serde_json::json!({"namespace":namespace,"persistence":"redis_aof","result":result})
    );
    Ok(())
}
pub fn run_at(url: &str, namespace: &str, config: Config) -> Result<serde_json::Value> {
    let (instrument, ts) = fixture()?;
    let mut session = Session::new(url, namespace, config.clone(), &instrument, ts)?;
    session.expected_token = Some(144870151);
    session.connected(1)?;
    for (i, p) in [
        6008, 6006, 6004, 6002, 6000, 6002, 6004, 6006, 6005, 6005, 6003, 6003, 6001, 5999, 6000,
    ]
    .iter()
    .enumerate()
    {
        let now = ts.as_u64() + (i as u64 + 1) * 1_000_000_000;
        session.full(full_snapshot(*p, now, 1), now)?;
    }
    session.gap()?;
    session.connected(2)?;
    let mut result = session.finish()?;
    result["reconnect_generation"] = 2.into();
    result["market_data_source"] = "synthetic".into();
    result["data_mode"] = "full".into();
    result["strategy_input"] = "KiteFullTick".into();
    drop(session);
    let recovery = super::recovery::inspect(url, namespace)?;
    result["native_replay_verified"] = recovery["native_replay_verified"].clone();
    anyhow::ensure!(
        kite_paper::worker::Handle::start(url.to_owned(), namespace.to_owned(), config).is_err(),
        "Existing paper session unexpectedly restarted"
    );
    result["restart_resubmission_blocked"] = true.into();
    Ok(result)
}

pub(crate) fn full_snapshot(
    p: i32,
    ts: u64,
    generation: u32,
) -> kite_adapter::mapping::market_data::Snapshot {
    use kite_adapter::websocket::models::{DepthLevel, FullFields, QuoteFields, Tick};
    let tick = Tick {
        instrument_token: 144870151,
        ltp_paise: p * 100,
        last_quantity: Some(1),
        cumulative_volume: Some(1000),
        quote_fields: Some(QuoteFields {
            average_price_paise: p * 100,
            total_buy_quantity: 100,
            total_sell_quantity: 100,
            open_paise: p * 100,
            high_paise: (p + 20) * 100,
            low_paise: (p - 20) * 100,
            close_paise: p * 100,
        }),
        full: Some(FullFields {
            last_trade_timestamp: Some((ts / 1_000_000_000) as u32),
            exchange_timestamp: Some((ts / 1_000_000_000) as u32),
            open_interest: 500,
            open_interest_day_high: 600,
            open_interest_day_low: 400,
            bids: std::array::from_fn(|i| DepthLevel {
                price_paise: (p - i as i32) * 100,
                quantity: 10,
                orders: 1,
            }),
            asks: std::array::from_fn(|i| DepthLevel {
                price_paise: (p + 1 + i as i32) * 100,
                quantity: 10,
                orders: 1,
            }),
        }),
    };
    kite_adapter::mapping::market_data::snapshot(
        &tick,
        chrono::DateTime::from_timestamp_nanos(ts as i64),
        generation,
    )
}

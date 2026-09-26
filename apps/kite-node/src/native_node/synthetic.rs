//! Synthetic market-data fixtures used only by offline tests and simulations.
use anyhow::Result;
use nautilus_core::UnixNanos;
use nautilus_model::instruments::FuturesContract;

pub fn fixture() -> Result<(FuturesContract, UnixNanos)> {
    let ts = UnixNanos::from(1_789_450_000_000_000_000_u64);
    let report = kite_adapter::preflight::Report {
        mode: "data_only",
        instrument_id: "CRUDEOIL26SEPFUT.MCX".into(),
        instrument_token: 144_870_151,
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

pub fn live_clock_fixture() -> Result<(FuturesContract, UnixNanos)> {
    let (mut instrument, _) = fixture()?;
    let now = nautilus_core::time::get_atomic_clock_realtime().get_time_ns();
    instrument.expiration_ns = (now.as_u64() + 86_400_000_000_000).into();
    Ok((instrument, now))
}

pub fn full_snapshot(
    price: i32,
    ts: u64,
    generation: u32,
) -> kite_adapter::mapping::market_data::Snapshot {
    full_snapshot_for(144_870_151, price, ts, generation)
}

pub fn full_snapshot_for(
    instrument_token: u32,
    price: i32,
    ts: u64,
    generation: u32,
) -> kite_adapter::mapping::market_data::Snapshot {
    use kite_adapter::websocket::models::{DepthLevel, FullFields, QuoteFields, Tick};
    let tick = Tick {
        instrument_token,
        ltp_paise: price * 100,
        last_quantity: Some(1),
        cumulative_volume: Some(1000),
        quote_fields: Some(QuoteFields {
            average_price_paise: price * 100,
            total_buy_quantity: 100,
            total_sell_quantity: 100,
            open_paise: price * 100,
            high_paise: (price + 20) * 100,
            low_paise: (price - 20) * 100,
            close_paise: price * 100,
        }),
        full: Some(FullFields {
            last_trade_timestamp: Some((ts / 1_000_000_000) as u32),
            exchange_timestamp: Some((ts / 1_000_000_000) as u32),
            open_interest: 500,
            open_interest_day_high: 600,
            open_interest_day_low: 400,
            bids: std::array::from_fn(|i| DepthLevel {
                price_paise: (price - i as i32) * 100,
                quantity: 10,
                orders: 1,
            }),
            asks: std::array::from_fn(|i| DepthLevel {
                price_paise: (price + 1 + i as i32) * 100,
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

//! Exact MCX price scaling; snapshot observations are not unique trades.
use crate::websocket::models::Tick;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

pub fn mcx_price(paise: i32) -> String {
    Decimal::new(i64::from(paise), 2).to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Tick>,
    pub instrument_token: u32,
    pub received_at_utc: DateTime<Utc>,
    pub connection_generation: u32,
    pub ltp: String,
    pub bid: Option<String>,
    pub ask: Option<String>,
    pub bid_size: Option<u32>,
    pub ask_size: Option<u32>,
    pub cumulative_volume: Option<u32>,
    pub open_interest: Option<u32>,
    pub exchange_timestamp: Option<u32>,
    pub source_fresh: bool,
}

pub fn is_fresh(source: Option<u32>, received: i64) -> bool {
    source.is_some_and(|s| {
        let age = received - i64::from(s);
        (-2..=10).contains(&age)
    })
}

pub fn snapshot(tick: &Tick, received: DateTime<Utc>, generation: u32) -> Snapshot {
    let source = tick.full.as_ref().and_then(|f| f.exchange_timestamp);
    Snapshot {
        raw: Some(tick.clone()),
        instrument_token: tick.instrument_token,
        received_at_utc: received,
        connection_generation: generation,
        ltp: mcx_price(tick.ltp_paise),
        bid: tick
            .full
            .as_ref()
            .and_then(|f| (f.bids[0].quantity > 0).then(|| mcx_price(f.bids[0].price_paise))),
        ask: tick
            .full
            .as_ref()
            .and_then(|f| (f.asks[0].quantity > 0).then(|| mcx_price(f.asks[0].price_paise))),
        bid_size: tick.full.as_ref().map(|f| f.bids[0].quantity),
        ask_size: tick.full.as_ref().map(|f| f.asks[0].quantity),
        cumulative_volume: tick.cumulative_volume,
        open_interest: tick.full.as_ref().map(|f| f.open_interest),
        exchange_timestamp: source,
        source_fresh: is_fresh(source, received.timestamp()),
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn price_scaling_is_exact() {
        assert_eq!(super::mcx_price(612345), "6123.45");
        assert_eq!(super::mcx_price(-125), "-1.25");
    }
    #[test]
    fn missing_old_and_future_source_times_are_not_fresh() {
        assert!(!super::is_fresh(None, 100));
        assert!(!super::is_fresh(Some(80), 100));
        assert!(!super::is_fresh(Some(110), 100));
        assert!(super::is_fresh(Some(99), 100));
    }
}

//! Bounded aggregate diagnostics; never contains credentials or account data.
use kite_strategy::config::Config;
use nautilus_model::{data::QuoteTick, identifiers::InstrumentId, types::Quantity};
use rust_decimal::Decimal;
use std::{cell::Cell, collections::BTreeMap};
#[derive(Default)]
pub struct Diagnostics {
    pub received_packets: u64,
    pub full_packets: u64,
    pub connections: u64,
    pub socket_gaps: u64,
    pub quality_suspensions: u64,
    pub watchdog_timeouts: u64,
    pub buffered_before_acceptance: u64,
    pub rejected: BTreeMap<String, u64>,
    pub max_source_age_ms: u64,
    pub max_queue_lag_ms: u64,
    pub max_checkpoint_ms: Cell<u64>,
}
impl Diagnostics {
    pub fn reject(&mut self, reason: &str) {
        *self.rejected.entry(reason.into()).or_default() += 1;
    }
    pub fn observe(&mut self, source: Option<u64>, received: u64, now: u64) {
        if let Some(source) = source {
            self.max_source_age_ms = self
                .max_source_age_ms
                .max(now.saturating_sub(source) / 1_000_000);
        }
        self.max_queue_lag_ms = self
            .max_queue_lag_ms
            .max(now.saturating_sub(received) / 1_000_000);
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
         "received_packets":self.received_packets,"full_packets":self.full_packets,"connections":self.connections,
         "socket_gaps":self.socket_gaps,"quality_suspensions":self.quality_suspensions,"watchdog_timeouts":self.watchdog_timeouts,
         "buffered_before_acceptance":self.buffered_before_acceptance,"rejected_by_reason":self.rejected,
         "max_source_age_ms":self.max_source_age_ms,"max_queue_lag_ms":self.max_queue_lag_ms,"max_checkpoint_ms":self.max_checkpoint_ms.get()
        })
    }
}
pub fn quote_reason(
    q: &QuoteTick,
    config: &Config,
    last_source: u64,
    last_received: u64,
    now: u64,
) -> Option<&'static str> {
    let bid = q.bid_price.as_decimal();
    let ask = q.ask_price.as_decimal();
    let source = q.ts_event.as_u64();
    let received = q.ts_init.as_u64();
    if q.instrument_id != InstrumentId::from("CRUDEOIL26SEPFUT.MCX") {
        return Some("wrong_instrument");
    }
    if bid <= Decimal::ZERO || ask <= Decimal::ZERO {
        return Some("nonpositive_price");
    }
    if ask < bid {
        return Some("crossed_book");
    }
    if !bid.fract().is_zero() || !ask.fract().is_zero() {
        return Some("invalid_tick_size");
    }
    if ask - bid > Decimal::from(config.max_spread_rupees) {
        return Some("spread_exceeded");
    }
    if q.bid_size < Quantity::from(1) || q.ask_size < Quantity::from(1) {
        return Some("insufficient_top_depth");
    }
    if source == 0 {
        return Some("missing_exchange_timestamp");
    }
    if source.saturating_sub(received) > 2_000_000_000 {
        return Some("future_exchange_timestamp");
    }
    if received.saturating_sub(source) > u64::from(config.max_age_seconds) * 1_000_000_000 {
        return Some("stale_at_receipt");
    }
    if source < last_source {
        return Some("backward_exchange_timestamp");
    }
    if received <= last_received {
        return Some("nonincreasing_receive_timestamp");
    }
    if now.saturating_sub(source) > u64::from(config.max_age_seconds) * 1_000_000_000 {
        return Some("stale_in_queue");
    }
    None
}

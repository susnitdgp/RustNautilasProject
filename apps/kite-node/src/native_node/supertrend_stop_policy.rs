//! Supertrend/MACD/VWAP decisions plus a fixed ATR stop re-entry block.
use super::{supertrend::Supertrend, supertrend_confirmation::Confirmation};
use nautilus_indicators::{average::MovingAverageType, volatility::atr::AverageTrueRange};
use serde::Serialize;
#[derive(Debug, Serialize)]
pub struct Reading {
    pub entry: i8,
    pub raw_entry: i8,
    pub atr: f64,
    pub blocked_direction: i8,
    pub supertrend: Option<f64>,
    pub confirmation: serde_json::Value,
}
#[derive(Debug)]
pub struct Policy {
    trend: Supertrend,
    confirmation: Confirmation,
    atr: AverageTrueRange,
    pub blocked_direction: i8,
    direction: i8,
    start: u64,
}
impl Policy {
    pub fn new(blocked: i8, start: u64) -> Self {
        Self {
            trend: Supertrend::new(),
            confirmation: Confirmation::new(),
            atr: AverageTrueRange::new(14, Some(MovingAverageType::Wilder), Some(true), None),
            blocked_direction: blocked,
            direction: 0,
            start,
        }
    }
    pub fn stopped(&mut self) {
        self.blocked_direction = self.direction;
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64, volume: f64, ts: u64) -> Reading {
        let result = self.trend.update(high, low, close);
        self.direction = result.map_or(0, |v| v.0);
        self.atr.update_raw(high, low, close);
        let (allowed, confirmation) = self.confirmation.update(high, low, close, volume, ts);
        if ts > self.start && self.direction != 0 && self.direction != self.blocked_direction {
            self.blocked_direction = 0;
        }
        let entry =
            if self.blocked_direction == 0 && self.atr.initialized && self.direction == allowed {
                allowed
            } else {
                0
            };
        Reading {
            entry,
            raw_entry: self.direction,
            atr: self.atr.value,
            blocked_direction: self.blocked_direction,
            supertrend: result.map(|v| v.1),
            confirmation,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stopped_trend_stays_blocked_until_direction_changes() {
        let mut p = Policy::new(0, 0);
        for i in 1..100 {
            p.update(101., 99., 100., 10., i);
        }
        p.stopped();
        assert_eq!(p.blocked_direction, -1);
        let r = p.update(101., 99., 100., 10., 100);
        assert_eq!(r.entry, 0);
        assert_eq!(r.blocked_direction, -1);
        let r = p.update(201., 199., 200., 10., 101);
        assert_eq!(r.raw_entry, 1);
        assert_eq!(r.blocked_direction, 0);
        p.stopped();
        assert_eq!(p.blocked_direction, 1);
        let r = p.update(201., 199., 200., 10., 102);
        assert_eq!(r.entry, 0);
        let r = p.update(51., 49., 50., 10., 103);
        assert_eq!(r.raw_entry, -1);
        assert_eq!(r.blocked_direction, 0);
    }
    #[test]
    fn previous_session_block_survives_historical_warmup() {
        let mut p = Policy::new(1, 200);
        for i in 1..100 {
            p.update(101., 99., 100., 10., i);
        }
        assert_eq!(p.blocked_direction, 1);
        p.update(201., 199., 200., 10., 100);
        assert_eq!(p.update(201., 199., 200., 10., 201).blocked_direction, 1);
        assert_eq!(p.update(51., 49., 50., 10., 202).blocked_direction, 0);
    }
}

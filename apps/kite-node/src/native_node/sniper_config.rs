//! Precision Sniper slot settings (JSON), shared by the backtest and the live runner.
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, FixedOffset, NaiveTime};
use kite_adapter::http::historical::{Interval, KiteInterval};
use serde::{Deserialize, Serialize};

pub const STRATEGY: &str = "sniper";

/// Live-runner settings.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LiveSettings {
    /// Reviewed exchange session calendar JSON (holidays, special sessions).
    pub session_calendar: String,
    /// Broker product; the production client only accepts MIS.
    pub product: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SniperConfig {
    pub strategy: String,
    pub symbol: String,
    pub instrument_token: u32,
    /// Candle minutes: 3, 5, 10, 15 or 30 (fetched natively from Kite).
    pub bar_minutes: u32,
    pub lots: u32,
    /// Lots closed at TP1 / TP2 (the rest at TP3 / stop). 0 = whole-position model.
    #[serde(default)]
    pub tp1_lots: u32,
    #[serde(default)]
    pub tp2_lots: u32,
    pub point_value: f64,
    /// Per-lot round-trip costs excluding brokerage (taxes, exchange, stamp), in points.
    pub round_trip_cost_points: f64,
    /// Flat brokerage per executed order (Zerodha ₹20).
    #[serde(default)]
    pub brokerage_per_order: f64,
    pub slippage_points_per_side: f64,
    pub live: LiveSettings,
    pub entries_until: NaiveTime,
    pub square_off: NaiveTime,
    /// IST windows [from, to) with no new entries (by signal-bar close), e.g. US data.
    #[serde(default)]
    pub entry_blackouts: Vec<Blackout>,
    #[serde(default)]
    pub params: sniper::Params,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Blackout {
    pub from: NaiveTime,
    pub to: NaiveTime,
}

impl SniperConfig {
    pub fn load(path: &str) -> Result<Self> {
        let raw = std::fs::read(path).with_context(|| format!("Cannot read {path}"))?;
        let c: Self = serde_json::from_slice(&raw).with_context(|| format!("Invalid sniper config {path}"))?;
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.strategy == STRATEGY, "strategy must be \"{STRATEGY}\"");
        ensure!(self.instrument_token > 0, "instrument_token required");
        ensure!([3, 5, 10, 15, 30].contains(&self.bar_minutes), "bar_minutes must be 3, 5, 10, 15 or 30");
        ensure!(self.params.timeframe_minutes == self.bar_minutes, "params.timeframe_minutes must equal bar_minutes");
        ensure!((1..=100).contains(&self.lots), "lots must be 1..100");
        ensure!(self.tp1_lots + self.tp2_lots < self.lots, "tp1_lots + tp2_lots must leave at least one lot for TP3");
        ensure!(
            self.params.tp1_close_fraction == 0.0 && self.params.tp2_close_fraction == 0.0,
            "set partial exits with tp1_lots / tp2_lots, not params fractions"
        );
        ensure!(self.brokerage_per_order >= 0.0, "brokerage_per_order must be >= 0");
        ensure!(self.point_value > 0.0, "point_value must be positive");
        ensure!(self.round_trip_cost_points >= 0.0 && self.slippage_points_per_side >= 0.0, "costs must be >= 0");
        ensure!(self.entries_until <= self.square_off, "entries_until must not be after square_off");
        ensure!(self.entry_blackouts.iter().all(|w| w.from < w.to), "entry_blackouts need from < to");
        ensure!(self.live.product == "MIS", "live.product must be MIS");
        self.params.validate().map_err(anyhow::Error::msg)
    }
    /// Engine params with the partial fractions derived from whole lots.
    pub fn engine_params(&self) -> sniper::Params {
        let mut p = self.params.clone();
        p.tp1_close_fraction = self.tp1_lots as f64 / self.lots as f64;
        p.tp2_close_fraction = self.tp2_lots as f64 / self.lots as f64;
        p
    }
    pub fn interval(&self) -> Interval {
        use KiteInterval as K;
        match self.bar_minutes {
            3 => K::ThreeMinute.native(),
            10 => K::TenMinute.native(),
            15 => K::FifteenMinute.native(),
            30 => K::ThirtyMinute.native(),
            _ => K::FiveMinute.native(),
        }
    }
}


fn ist(ts: i64) -> DateTime<FixedOffset> {
    DateTime::from_timestamp(ts, 0).expect("timestamp").with_timezone(&FixedOffset::east_opt(19_800).expect("IST"))
}

impl SniperConfig {
    /// New entries allowed for a signal bar closing at `close_ts` (epoch s): inside
    /// 09:00..entries_until IST and outside every blackout.
    pub fn entries_allowed_at(&self, close_ts: i64) -> bool {
        let t = ist(close_ts).time();
        t >= NaiveTime::from_hms_opt(9, 0, 0).expect("time")
            && t < self.entries_until
            && !self.entry_blackouts.iter().any(|w| t >= w.from && t < w.to)
    }
    /// Square-off due for a bar closing at `close_ts`.
    pub fn square_off_due(&self, close_ts: i64) -> bool {
        ist(close_ts).time() >= self.square_off
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SHIPPED: &str = include_str!("../../../../config/sniper-crudeoilm.json");
    #[test]
    fn shipped_config_is_the_three_lot_live_setup() {
        let c: SniperConfig = serde_json::from_str(SHIPPED).unwrap();
        c.validate().unwrap();
        assert_eq!((c.lots, c.tp1_lots, c.tp2_lots, c.bar_minutes), (3, 1, 1, 3));
        let p = c.engine_params();
        assert!((p.tp1_close_fraction - 1.0 / 3.0).abs() < 1e-12 && (p.tp2_close_fraction - 1.0 / 3.0).abs() < 1e-12);
        assert_eq!(p.resolve().preset, sniper::params::Preset::Aggressive);
        assert_eq!(p.grade_filter, sniper::params::GradeFilter::AOrBetter);
        let mut bad = c.clone();
        bad.tp2_lots = 2;
        assert!(bad.validate().is_err(), "TP1 + TP2 must leave a lot for TP3");
    }
    #[test]
    fn entry_window_and_blackout() {
        let c: SniperConfig = serde_json::from_str(SHIPPED).unwrap();
        // 2026-10-09 IST times as epoch seconds
        let at = |h: u32, m: u32| chrono::NaiveDate::from_ymd_opt(2026, 10, 9).unwrap().and_hms_opt(h, m, 0).unwrap().and_utc().timestamp() - 19_800;
        assert!(c.entries_allowed_at(at(10, 0)));
        assert!(c.entry_blackouts.is_empty(), "shipped without blackouts (2.26.2)");
        assert!(c.entries_allowed_at(at(18, 0)), "no US data blackout");
        let mut b = c.clone();
        b.entry_blackouts = vec![Blackout { from: NaiveTime::from_hms_opt(17, 30, 0).unwrap(), to: NaiveTime::from_hms_opt(19, 30, 0).unwrap() }];
        assert!(!b.entries_allowed_at(at(18, 0)), "blackout blocks entries");
        assert!(b.entries_allowed_at(at(19, 30)), "blackout ends at 19:30");
        assert!(!c.entries_allowed_at(at(23, 0)));
        assert!(c.square_off_due(at(23, 15)) && !c.square_off_due(at(23, 12)));
    }
}

//! Tunable rules. Every field has a default, so `{}` is a valid parameter set.
use chrono::NaiveTime;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Params {
    /// Wilder ATR length on 1m bars.
    pub atr_length: usize,
    /// Bars that form the base (the coil) before the breakout bar.
    pub base_bars: usize,
    /// Base high-low range must be at most this many ATRs.
    pub base_max_atr: f64,
    /// Breakout bar range must be at least this many ATRs.
    pub breakout_min_atr: f64,
    /// Breakout bar must close in the top (long) / bottom (short) part of its
    /// range: 0.75 = top 25 %.
    pub close_position: f64,
    /// Volume median window (bars before the breakout bar).
    pub volume_lookback: usize,
    /// Breakout bar volume must be at least this multiple of the median.
    pub volume_mult: f64,
    /// Opening range length from the session's first bar, minutes.
    pub opening_range_minutes: i64,
    /// Long only above session VWAP, short only below it.
    pub require_vwap: bool,
    /// Next opposing level (previous-day / opening-range high/low) must be at least
    /// `room_mult` × target distance away. 0 disables.
    pub room_mult: f64,
    /// Stop just beyond the base, this many points past its edge.
    pub stop_buffer_points: f64,
    /// Skip the trade when the stop is wider than this (points).
    pub stop_cap_points: f64,
    /// Never use a stop tighter than this (points); it widens the stop.
    pub stop_min_points: f64,
    /// Target at this multiple of the initial risk.
    pub target_r: f64,
    /// Move the stop to entry ± `breakeven_offset_points` once price reaches this R.
    pub breakeven_r: f64,
    pub breakeven_offset_points: f64,
    /// Trail the stop behind the last `trail_swing_bars` bars' low (long) / high
    /// (short) once price has reached this R. 0 disables trailing.
    pub trail_after_r: f64,
    pub trail_swing_bars: usize,
    /// Exit at a bar close if the trade has not reached `time_stop_min_r` within
    /// `time_stop_bars` bars. 0 bars disables.
    pub time_stop_bars: u32,
    pub time_stop_min_r: f64,
    /// New entries only for signal bars closing in [from, to) IST.
    pub entry_from: NaiveTime,
    pub entry_to: NaiveTime,
    /// Daily limits (reset each IST day).
    pub max_trades_per_day: u32,
    pub max_consecutive_losses: u32,
    /// Stop for the day once the day's net points are at or below -limit. 0 = off.
    pub daily_loss_limit_points: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            atr_length: 14,
            base_bars: 10,
            base_max_atr: 1.5,
            breakout_min_atr: 1.2,
            close_position: 0.75,
            volume_lookback: 20,
            volume_mult: 2.0,
            opening_range_minutes: 15,
            require_vwap: true,
            room_mult: 1.5,
            stop_buffer_points: 1.0,
            stop_cap_points: 25.0,
            stop_min_points: 6.0,
            target_r: 1.5,
            breakeven_r: 1.0,
            breakeven_offset_points: 2.0,
            trail_after_r: 1.0,
            trail_swing_bars: 3,
            time_stop_bars: 5,
            time_stop_min_r: 0.5,
            entry_from: NaiveTime::from_hms_opt(9, 15, 0).expect("time"),
            entry_to: NaiveTime::from_hms_opt(23, 0, 0).expect("time"),
            max_trades_per_day: 8,
            max_consecutive_losses: 3,
            daily_loss_limit_points: 0.0,
        }
    }
}

impl Params {
    pub fn validate(&self) -> Result<(), String> {
        let positive = [
            ("base_max_atr", self.base_max_atr),
            ("breakout_min_atr", self.breakout_min_atr),
            ("volume_mult", self.volume_mult),
            ("stop_cap_points", self.stop_cap_points),
            ("target_r", self.target_r),
        ];
        for (name, v) in positive {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("{name} must be positive"));
            }
        }
        let non_negative = [
            ("room_mult", self.room_mult),
            ("stop_buffer_points", self.stop_buffer_points),
            ("stop_min_points", self.stop_min_points),
            ("breakeven_r", self.breakeven_r),
            ("breakeven_offset_points", self.breakeven_offset_points),
            ("trail_after_r", self.trail_after_r),
            ("time_stop_min_r", self.time_stop_min_r),
            ("daily_loss_limit_points", self.daily_loss_limit_points),
        ];
        for (name, v) in non_negative {
            if !(v.is_finite() && v >= 0.0) {
                return Err(format!("{name} must be zero or positive"));
            }
        }
        if self.atr_length < 2 || self.base_bars < 2 || self.volume_lookback < 5 {
            return Err("atr_length/base_bars need at least 2, volume_lookback at least 5".into());
        }
        if !(0.5..=1.0).contains(&self.close_position) {
            return Err("close_position must be 0.5..1.0".into());
        }
        if self.stop_min_points > self.stop_cap_points {
            return Err("stop_min_points must not exceed stop_cap_points".into());
        }
        if self.trail_after_r > 0.0 && self.trail_swing_bars == 0 {
            return Err("trail_swing_bars must be at least 1 when trailing".into());
        }
        if self.entry_from >= self.entry_to {
            return Err("entry_from must be before entry_to".into());
        }
        if self.max_trades_per_day == 0 {
            return Err("max_trades_per_day must be at least 1".into());
        }
        Ok(())
    }
}

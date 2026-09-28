//! Confirmed-bar strategy engine for MCX Crude PURE Squeeze Momentum v2.28.3.
//!
//! Trading state mutates only in `update_confirmed`. `preview_live` is read-only
//! and exists solely for the tick-by-tick dashboard.
use anyhow::{Result, ensure};
use chrono::{FixedOffset, Timelike};
use serde::{Deserialize, Serialize};

use super::{
    session_calendar::Calendar,
    squeeze_momentum_indicator::{Config as IndicatorConfig, Engine as IndicatorEngine, Values},
    strategy_session::Session,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DisplaySettings {
    pub show_markers: bool,
    pub show_dashboard: bool,
    pub shade_outside: bool,
    pub outside_session_color: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub sqz_length: usize,
    pub sqz_length_kc: usize,
    pub sqz_mult_kc: f64,
    pub sqz_use_true_range: bool,
    pub entry_strength_bars: usize,
    pub sqz_weak_bars_req: usize,
    pub sqz_transition_pct: f64,
    pub session_timezone: String,
    pub allow_entries_only_in_session: bool,
    pub force_flat_at_session_end: bool,
    pub auto_sq_off_hour: u32,
    pub auto_sq_off_minute: u32,
    pub session: Session,
    pub display: DisplaySettings,
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        self.indicator_config().validate()?;
        ensure!(
            (1..=3).contains(&self.entry_strength_bars),
            "Entry Strengthening Bars must be 1..3"
        );
        ensure!(
            (1..=5).contains(&self.sqz_weak_bars_req),
            "Exit Consecutive Weakening Bars must be 1..5"
        );
        ensure!(
            self.sqz_transition_pct.is_finite() && (20.0..=95.0).contains(&self.sqz_transition_pct),
            "Exit Transition Toward Zero % must be 20..95"
        );
        ensure!(
            self.session_timezone == "Asia/Kolkata",
            "Pure SQZ currently requires session_timezone=Asia/Kolkata"
        );
        ensure!(
            self.auto_sq_off_hour <= 23,
            "Day-End Square-Off Hour must be 0..23"
        );
        ensure!(
            self.auto_sq_off_minute <= 59,
            "Day-End Square-Off Minute must be 0..59"
        );
        ensure!(
            !self.display.outside_session_color.trim().is_empty(),
            "outside_session_color cannot be empty"
        );
        self.session.validate()
    }

    pub fn indicator_config(&self) -> IndicatorConfig {
        IndicatorConfig {
            bb_length: self.sqz_length,
            kc_length: self.sqz_length_kc,
            kc_mult: self.sqz_mult_kc,
            use_true_range: self.sqz_use_true_range,
        }
    }

    pub fn required_warmup(&self) -> usize {
        (self.sqz_length_kc * 2 + 4).max(self.sqz_length + 4)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Action {
    Buy,
    Sell,
    Short,
    Cover,
}

impl Action {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
            Self::Short => "SHORT",
            Self::Cover => "COVER",
        }
    }

    pub const fn target(self) -> i8 {
        match self {
            Self::Buy => 1,
            Self::Short => -1,
            Self::Sell | Self::Cover => 0,
        }
    }

    pub const fn reason(self) -> &'static str {
        match self {
            Self::Buy => "sqz_strength_long",
            Self::Short => "sqz_strength_short",
            Self::Sell => "sqz_long_exit",
            Self::Cover => "sqz_short_exit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MomentumState {
    Warmup,
    PositiveRising,
    PositiveFalling,
    NegativeFalling,
    NegativeRising,
    Zero,
}

impl MomentumState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Warmup => "WARMUP",
            Self::PositiveRising => "POSITIVE / RISING",
            Self::PositiveFalling => "POSITIVE / FALLING",
            Self::NegativeFalling => "NEGATIVE / FALLING",
            Self::NegativeRising => "NEGATIVE / RISING",
            Self::Zero => "ZERO / FLAT",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Observation {
    pub bar_close_ns: u64,
    pub close: f64,
    pub confirmed: bool,
    pub in_session: bool,
    pub ready: bool,
    pub value: f64,
    pub squeeze_on: bool,
    pub squeeze_off: bool,
    pub squeeze_no: bool,
    pub momentum_state: MomentumState,
    pub wave_side: i8,
    pub wave_used: bool,
    pub entry_ready: bool,
    pub strengthening_count: usize,
    pub weakening_count: usize,
    pub retracement_pct: f64,
    pub extreme: Option<f64>,
    pub position: i8,
    pub action: Option<Action>,
    pub force_flat_event: bool,
    pub exit_zero_cross: bool,
}

#[derive(Debug, Clone)]
pub struct Engine {
    pub settings: Settings,
    calendar: Calendar,
    indicator: IndicatorEngine,
    long_wave_used: bool,
    short_wave_used: bool,
    long_peak: Option<f64>,
    short_trough: Option<f64>,
    long_weak_bars: usize,
    short_weak_bars: usize,
    last_bar_close_ns: u64,
}

impl Engine {
    pub fn new(settings: Settings, calendar: Calendar) -> Result<Self> {
        settings.validate()?;
        calendar.validate()?;
        Ok(Self {
            indicator: IndicatorEngine::new(settings.indicator_config())?,
            settings,
            calendar,
            long_wave_used: false,
            short_wave_used: false,
            long_peak: None,
            short_trough: None,
            long_weak_bars: 0,
            short_weak_bars: 0,
            last_bar_close_ns: 0,
        })
    }

    pub fn rebuild_empty(&self) -> Result<Self> {
        Self::new(self.settings.clone(), self.calendar.clone())
    }

    pub fn in_session(&self, open_ns: u64) -> Result<bool> {
        self.settings.session.contains(open_ns, &self.calendar)
    }

    fn strength_ready(&self, values: Values, side: i8) -> bool {
        match (side, self.settings.entry_strength_bars) {
            (1, 1) => values.long_strength1,
            (1, 2) => values.long_strength2,
            (1, 3) => values.long_strength3,
            (-1, 1) => values.short_strength1,
            (-1, 2) => values.short_strength2,
            (-1, 3) => values.short_strength3,
            _ => false,
        }
    }

    fn day_end_sqoff_bar(&self, in_session: bool, bar_close_ns: u64) -> bool {
        if !self.settings.force_flat_at_session_end || !in_session {
            return false;
        }
        let local = chrono::DateTime::from_timestamp_nanos(bar_close_ns as i64)
            .with_timezone(&FixedOffset::east_opt(19_800).expect("IST"));
        local.hour() == self.settings.auto_sq_off_hour
            && local.minute() == self.settings.auto_sq_off_minute
    }

    fn state_for(&self, values: Values) -> MomentumState {
        if !values.ready {
            MomentumState::Warmup
        } else if values.value > 0.0 {
            if values.long_strength1 {
                MomentumState::PositiveRising
            } else {
                MomentumState::PositiveFalling
            }
        } else if values.value < 0.0 {
            if values.short_strength1 {
                MomentumState::NegativeFalling
            } else {
                MomentumState::NegativeRising
            }
        } else {
            MomentumState::Zero
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn observation(
        &self,
        values: Values,
        bar_close_ns: u64,
        close: f64,
        confirmed: bool,
        in_session: bool,
        position: i8,
        live_preview: bool,
    ) -> Observation {
        if !values.ready {
            return Observation {
                bar_close_ns,
                close,
                confirmed,
                in_session,
                ready: false,
                value: 0.0,
                squeeze_on: false,
                squeeze_off: false,
                squeeze_no: false,
                momentum_state: MomentumState::Warmup,
                wave_side: 0,
                wave_used: false,
                entry_ready: false,
                strengthening_count: 0,
                weakening_count: 0,
                retracement_pct: 0.0,
                extreme: None,
                position,
                action: None,
                force_flat_event: false,
                exit_zero_cross: false,
            };
        }

        let wave_side = if values.value > 0.0 {
            1
        } else if values.value < 0.0 {
            -1
        } else {
            0
        };
        let long_strength_count = if values.long_strength3 {
            3
        } else if values.long_strength2 {
            2
        } else if values.long_strength1 {
            1
        } else {
            0
        };
        let short_strength_count = if values.short_strength3 {
            3
        } else if values.short_strength2 {
            2
        } else if values.short_strength1 {
            1
        } else {
            0
        };
        let long_live_peak = if position == 1 {
            Some(self.long_peak.map_or(values.value, |p| p.max(values.value)))
        } else {
            None
        };
        let short_live_trough = if position == -1 {
            Some(
                self.short_trough
                    .map_or(values.value, |p| p.min(values.value)),
            )
        } else {
            None
        };
        let long_live_weak = if position == 1 && values.long_weak_bar {
            self.long_weak_bars + usize::from(live_preview)
        } else if position == 1 {
            self.long_weak_bars
        } else {
            0
        };
        let short_live_weak = if position == -1 && values.short_weak_bar {
            self.short_weak_bars + usize::from(live_preview)
        } else if position == -1 {
            self.short_weak_bars
        } else {
            0
        };
        let long_decay = long_live_peak
            .filter(|p| *p > 0.0)
            .map_or(0.0, |p| ((p - values.value) / p * 100.0).max(0.0));
        let short_decay = short_live_trough
            .filter(|p| *p < 0.0)
            .map_or(0.0, |p| ((values.value - p) / p.abs() * 100.0).max(0.0));
        let wave_used = if values.value > 0.0 {
            self.long_wave_used
        } else if values.value < 0.0 {
            self.short_wave_used
        } else {
            false
        };
        let entry_session_ok = !self.settings.allow_entries_only_in_session || in_session;
        let entry_ready = position == 0
            && entry_session_ok
            && !wave_used
            && self.strength_ready(values, wave_side);

        Observation {
            bar_close_ns,
            close,
            confirmed,
            in_session,
            ready: true,
            value: values.value,
            squeeze_on: values.squeeze_on,
            squeeze_off: values.squeeze_off,
            squeeze_no: values.squeeze_no,
            momentum_state: self.state_for(values),
            wave_side,
            wave_used,
            entry_ready,
            strengthening_count: if wave_side > 0 {
                long_strength_count
            } else if wave_side < 0 {
                short_strength_count
            } else {
                0
            },
            weakening_count: if position > 0 {
                long_live_weak
            } else if position < 0 {
                short_live_weak
            } else {
                0
            },
            retracement_pct: if position > 0 {
                long_decay
            } else if position < 0 {
                short_decay
            } else {
                0.0
            },
            extreme: if position > 0 {
                long_live_peak
            } else if position < 0 {
                short_live_trough
            } else {
                None
            },
            position,
            action: None,
            force_flat_event: false,
            exit_zero_cross: false,
        }
    }

    /// Read-only forming-candle calculation for the dashboard. Never mutates
    /// wave use, peaks, weakening counters, or trade state.
    pub fn preview_live(
        &self,
        high: f64,
        low: f64,
        close: f64,
        bar_close_ns: u64,
        bar_ns: u64,
        position: i8,
    ) -> Result<Observation> {
        let open_ns = bar_close_ns.saturating_sub(bar_ns);
        let in_session = self.in_session(open_ns)?;
        let values = self.indicator.preview(high, low, close);
        Ok(self.observation(
            values,
            bar_close_ns,
            close,
            false,
            in_session,
            position,
            true,
        ))
    }

    /// The only method allowed to mutate trading state. Call once per completed
    /// candle, after the candle is confirmed.
    #[allow(clippy::too_many_arguments)]
    pub fn update_confirmed(
        &mut self,
        high: f64,
        low: f64,
        close: f64,
        bar_close_ns: u64,
        bar_ns: u64,
        position: i8,
        allow_action: bool,
    ) -> Result<Observation> {
        ensure!(
            bar_ns > 0
                && bar_close_ns >= bar_ns
                && bar_close_ns > self.last_bar_close_ns
                && bar_close_ns.is_multiple_of(bar_ns),
            "SQZ bars must be ordered completed candles"
        );
        ensure!(
            [high, low, close].iter().all(|x| x.is_finite() && *x > 0.0)
                && high >= close
                && close >= low,
            "Invalid SQZ OHLC"
        );
        self.last_bar_close_ns = bar_close_ns;
        let open_ns = bar_close_ns - bar_ns;
        let in_session = self.in_session(open_ns)?;
        let values = self.indicator.update(high, low, close);
        if !values.ready {
            return Ok(self.observation(
                values,
                bar_close_ns,
                close,
                true,
                in_session,
                position,
                false,
            ));
        }

        // Exact Pine wave reset: sign reaching/crossing zero re-arms that side.
        if values.value <= 0.0 {
            self.long_wave_used = false;
        }
        if values.value >= 0.0 {
            self.short_wave_used = false;
        }

        // Exact Pine confirmed-state tracking, before signal evaluation.
        if position == 1 {
            self.long_peak = Some(self.long_peak.map_or(values.value, |p| p.max(values.value)));
            self.long_weak_bars = if values.long_weak_bar {
                self.long_weak_bars + 1
            } else {
                0
            };
        } else {
            self.long_peak = None;
            self.long_weak_bars = 0;
        }
        if position == -1 {
            self.short_trough = Some(
                self.short_trough
                    .map_or(values.value, |p| p.min(values.value)),
            );
            self.short_weak_bars = if values.short_weak_bar {
                self.short_weak_bars + 1
            } else {
                0
            };
        } else {
            self.short_trough = None;
            self.short_weak_bars = 0;
        }

        let long_decay = self
            .long_peak
            .filter(|p| *p > 0.0)
            .map_or(0.0, |p| ((p - values.value) / p * 100.0).max(0.0));
        let short_decay = self
            .short_trough
            .filter(|p| *p < 0.0)
            .map_or(0.0, |p| ((values.value - p) / p.abs() * 100.0).max(0.0));
        let long_zero_exit = position == 1 && values.value <= 0.0;
        let short_zero_exit = position == -1 && values.value >= 0.0;
        let long_transition_exit = position == 1
            && self.long_weak_bars >= self.settings.sqz_weak_bars_req
            && long_decay >= self.settings.sqz_transition_pct;
        let short_transition_exit = position == -1
            && self.short_weak_bars >= self.settings.sqz_weak_bars_req
            && short_decay >= self.settings.sqz_transition_pct;
        let entry_session_ok = !self.settings.allow_entries_only_in_session || in_session;
        let long_entry = position == 0
            && entry_session_ok
            && !self.long_wave_used
            && self.strength_ready(values, 1);
        let short_entry = position == 0
            && entry_session_ok
            && !self.short_wave_used
            && self.strength_ready(values, -1);
        let day_end = self.day_end_sqoff_bar(in_session, bar_close_ns);

        let mut force_flat_event = false;
        let mut exit_zero_cross = false;
        let action = if allow_action {
            // Pine priority: SQZ exits -> day-end exits -> entries.
            if long_zero_exit || long_transition_exit {
                exit_zero_cross = long_zero_exit;
                Some(Action::Sell)
            } else if short_zero_exit || short_transition_exit {
                exit_zero_cross = short_zero_exit;
                Some(Action::Cover)
            } else if day_end && position == 1 {
                force_flat_event = true;
                Some(Action::Sell)
            } else if day_end && position == -1 {
                force_flat_event = true;
                Some(Action::Cover)
            } else if long_entry {
                Some(Action::Buy)
            } else if short_entry {
                Some(Action::Short)
            } else {
                None
            }
        } else {
            None
        };

        match action {
            Some(Action::Buy) => {
                self.long_wave_used = true;
                self.long_peak = Some(values.value);
                self.long_weak_bars = 0;
            }
            Some(Action::Short) => {
                self.short_wave_used = true;
                self.short_trough = Some(values.value);
                self.short_weak_bars = 0;
            }
            Some(Action::Sell) => {
                self.long_peak = None;
                self.long_weak_bars = 0;
            }
            Some(Action::Cover) => {
                self.short_trough = None;
                self.short_weak_bars = 0;
            }
            None => {}
        }

        let mut observation = self.observation(
            values,
            bar_close_ns,
            close,
            true,
            in_session,
            position,
            false,
        );
        observation.action = action;
        observation.force_flat_event = force_flat_event;
        observation.exit_zero_cross = exit_zero_cross;
        // Dashboard should show post-dispatch wave state exactly like Pine.
        observation.wave_used = if values.value > 0.0 {
            self.long_wave_used
        } else if values.value < 0.0 {
            self.short_wave_used
        } else {
            false
        };
        Ok(observation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline_settings() -> Settings {
        serde_json::from_value(serde_json::json!({
            "sqz_length":20,
            "sqz_length_kc":20,
            "sqz_mult_kc":1.5,
            "sqz_use_true_range":true,
            "entry_strength_bars":2,
            "sqz_weak_bars_req":2,
            "sqz_transition_pct":70.0,
            "session_timezone":"Asia/Kolkata",
            "allow_entries_only_in_session":true,
            "force_flat_at_session_end":true,
            "auto_sq_off_hour":23,
            "auto_sq_off_minute":15,
            "session":{"start":"09:00:00","end":"23:15:00","days":"23456","reset_daily":false},
            "display":{"show_markers":true,"show_dashboard":true,"shade_outside":true,"outside_session_color":"gray@86"}
        })).unwrap()
    }

    #[test]
    fn json_baseline_matches_v2283_defaults() {
        let settings = baseline_settings();
        settings.validate().unwrap();
        assert_eq!(settings.entry_strength_bars, 2);
        assert_eq!(settings.sqz_weak_bars_req, 2);
        assert_eq!(settings.sqz_transition_pct, 70.0);
        assert!(settings.force_flat_at_session_end);
        assert_eq!(settings.auto_sq_off_hour, 23);
        assert_eq!(settings.auto_sq_off_minute, 15);
    }

    #[test]
    fn live_preview_does_not_change_confirmed_actions() {
        let settings = baseline_settings();
        let calendar = super::super::session_calendar::fixture();
        let mut with_preview = Engine::new(settings.clone(), calendar.clone()).unwrap();
        let mut close_only = Engine::new(settings, calendar.clone()).unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 22).unwrap();
        let (start, _) = calendar.bounds(date).unwrap();
        let bar_ns = 300_000_000_000u64;
        let mut position_preview = 0i8;
        let mut position_close = 0i8;
        let mut actions_preview = Vec::new();
        let mut actions_close = Vec::new();

        for i in 0..165u64 {
            let phase = i % 48;
            let base = if phase < 24 {
                9000.0 + phase as f64 * 8.0
            } else {
                9000.0 + (48 - phase) as f64 * 8.0
            };
            let high = base + 12.0;
            let low = base - 12.0;
            let close = if i % 3 == 0 { base + 5.0 } else { base - 3.0 };
            let bar_close = start + (i + 1) * bar_ns;

            // Several forming-candle updates at deliberately different prices.
            for delta in [-9.0, 6.0, 2.0] {
                let _ = with_preview
                    .preview_live(
                        high.max(close + delta),
                        low.min(close + delta),
                        close + delta,
                        bar_close,
                        bar_ns,
                        position_preview,
                    )
                    .unwrap();
            }

            let a = with_preview
                .update_confirmed(high, low, close, bar_close, bar_ns, position_preview, true)
                .unwrap();
            let b = close_only
                .update_confirmed(high, low, close, bar_close, bar_ns, position_close, true)
                .unwrap();
            assert_eq!(a.action, b.action);
            assert_eq!(a.wave_used, b.wave_used);
            assert_eq!(a.weakening_count, b.weakening_count);
            assert!((a.retracement_pct - b.retracement_pct).abs() < 1e-12);
            if let Some(action) = a.action {
                actions_preview.push((bar_close, action));
                position_preview = action.target();
            }
            if let Some(action) = b.action {
                actions_close.push((bar_close, action));
                position_close = action.target();
            }
        }
        assert!(!actions_preview.is_empty());
        assert_eq!(actions_preview, actions_close);
        assert_eq!(position_preview, position_close);
    }
}

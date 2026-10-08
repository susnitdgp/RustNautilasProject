//! Smart Money Breakout Channels (SMBC) v1.7 non-visual strategy engine.
use super::{session_calendar::Calendar, strategy_session::Session};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DisplaySettings {
    pub show_dashboard: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub overlap: bool,
    pub strong_closes_only: bool,
    pub normalization_length: usize,
    pub box_detection_length: usize,
    pub enable_longs: bool,
    pub enable_shorts: bool,
    pub opposite_breakout: String,
    pub intrabar_exit: bool,
    pub entry_cooldown_bars: usize,
    pub stop_mode: String,
    pub atr_length: usize,
    pub sl_atr_mult: f64,
    pub wick_lookback: usize,
    pub wick_atr_buffer: f64,
    pub min_stop_points: f64,
    pub max_stop_points: f64,
    pub target_mode: String,
    pub reward_risk: f64,
    pub tp_atr_mult: f64,
    pub max_target_points: f64,
    pub breakeven_r: f64,
    pub trail_mode: String,
    pub trail_atr_mult: f64,
    pub trail_lookback: usize,
    pub trail_activate_r: f64,
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
        ensure!(
            self.normalization_length > 0 && self.box_detection_length > 0,
            "channel lengths must be > 0"
        );
        ensure!(
            self.atr_length > 0 && self.wick_lookback > 0 && self.trail_lookback > 0,
            "lookbacks must be > 0"
        );
        ensure!(
            matches!(
                self.opposite_breakout.as_str(),
                "Reverse" | "Exit Only" | "Ignore"
            ),
            "invalid opposite_breakout"
        );
        ensure!(
            matches!(
                self.stop_mode.as_str(),
                "ATR" | "Candle Wick" | "Wick + ATR Buffer"
            ),
            "invalid stop_mode"
        );
        ensure!(
            matches!(self.target_mode.as_str(), "R:R" | "ATR" | "None"),
            "invalid target_mode"
        );
        ensure!(
            matches!(self.trail_mode.as_str(), "Off" | "ATR Trail" | "Wick Trail"),
            "invalid trail_mode"
        );
        ensure!(
            self.sl_atr_mult > 0.0
                && self.reward_risk > 0.0
                && self.tp_atr_mult > 0.0
                && self.trail_atr_mult > 0.0,
            "multipliers must be > 0"
        );
        ensure!(
            self.min_stop_points >= 0.0
                && self.max_stop_points >= 0.0
                && self.max_target_points >= 0.0,
            "distance limits must be >= 0"
        );
        ensure!(
            self.max_stop_points == 0.0 || self.max_stop_points >= self.min_stop_points,
            "max_stop_points must be 0 or >= min_stop_points"
        );
        ensure!(
            self.breakeven_r >= 0.0 && self.trail_activate_r >= 0.0,
            "R thresholds must be >= 0"
        );
        ensure!(
            self.session_timezone == "Asia/Kolkata",
            "only Asia/Kolkata is supported"
        );
        ensure!(
            self.auto_sq_off_hour <= 23 && self.auto_sq_off_minute <= 59,
            "invalid square-off time"
        );
        self.session.validate()?;
        Ok(())
    }
    pub fn required_warmup(&self) -> usize {
        self.normalization_length + self.box_detection_length + 32
    }
    pub fn clamp_stop_distance(&self, d: f64) -> f64 {
        let d = d.max(self.min_stop_points);
        if self.max_stop_points > 0.0 {
            d.min(self.max_stop_points)
        } else {
            d
        }
    }
    pub fn cap_target_distance(&self, d: f64) -> f64 {
        if self.max_target_points > 0.0 {
            d.min(self.max_target_points)
        } else {
            d
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PriceBar {
    high: f64,
    low: f64,
}
#[derive(Debug, Clone, Copy)]
struct Channel {
    top: f64,
    bottom: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Observation {
    pub bar_close_ns: u64,
    pub confirmed: bool,
    pub close: f64,
    pub atr: Option<f64>,
    pub wick_low: Option<f64>,
    pub wick_high: Option<f64>,
    pub trail_low: Option<f64>,
    pub trail_high: Option<f64>,
    pub channel_top: Option<f64>,
    pub channel_bottom: Option<f64>,
    pub channel_count: usize,
    pub bullish_breakout: bool,
    pub bearish_breakout: bool,
    pub in_session: bool,
    pub eod_hit: bool,
    pub can_enter: bool,
}

#[derive(Debug)]
pub struct Engine {
    settings: Settings,
    calendar: Calendar,
    bars: VecDeque<PriceBar>,
    normalized: VecDeque<f64>,
    vol: VecDeque<f64>,
    channels: Vec<Channel>,
    prev_upper: Option<f64>,
    prev_lower: Option<f64>,
    since_lower_cross: usize,
    atr: Option<f64>,
    atr_seed_sum: f64,
    atr_seed_count: usize,
    prev_close: Option<f64>,
    bar_index: u64,
}

impl Engine {
    pub fn new(settings: Settings, calendar: Calendar) -> Result<Self> {
        settings.validate()?;
        Ok(Self {
            settings,
            calendar,
            bars: VecDeque::new(),
            normalized: VecDeque::new(),
            vol: VecDeque::new(),
            channels: Vec::new(),
            prev_upper: None,
            prev_lower: None,
            since_lower_cross: 1,
            atr: None,
            atr_seed_sum: 0.0,
            atr_seed_count: 0,
            prev_close: None,
            bar_index: 0,
        })
    }
    pub fn rebuild_empty(&self) -> Result<Self> {
        Self::new(self.settings.clone(), self.calendar.clone())
    }

    fn update_atr(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let tr = self.prev_close.map_or(high - low, |pc| {
            (high - low).max((high - pc).abs()).max((low - pc).abs())
        });
        self.prev_close = Some(close);
        if self.atr.is_none() {
            self.atr_seed_sum += tr;
            self.atr_seed_count += 1;
            if self.atr_seed_count >= self.settings.atr_length {
                self.atr = Some(self.atr_seed_sum / self.settings.atr_length as f64);
            }
        } else {
            let n = self.settings.atr_length as f64;
            self.atr = Some((self.atr.unwrap() * (n - 1.0) + tr) / n);
        }
        self.atr
    }
    fn range(&self, n: usize) -> Option<(f64, f64)> {
        if self.bars.len() < n {
            return None;
        }
        let mut hi = f64::NEG_INFINITY;
        let mut lo = f64::INFINITY;
        for b in self.bars.iter().rev().take(n) {
            hi = hi.max(b.high);
            lo = lo.min(b.low);
        }
        Some((hi, lo))
    }
    fn stdev14(&self) -> Option<f64> {
        if self.normalized.len() < 14 {
            return None;
        }
        let vals: Vec<_> = self.normalized.iter().rev().take(14).copied().collect();
        let mean = vals.iter().sum::<f64>() / 14.0;
        Some((vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / 14.0).sqrt())
    }
    fn extremum_back(&self, highest: bool, n: usize) -> Option<usize> {
        if self.vol.len() < n {
            return None;
        }
        let mut best = if highest {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
        let mut back = 0;
        for (i, v) in self.vol.iter().rev().take(n).enumerate() {
            if (highest && *v > best) || (!highest && *v < best) {
                best = *v;
                back = i;
            }
        }
        Some(back)
    }
    fn can_create(&self, top: f64, bottom: f64) -> bool {
        !self
            .channels
            .iter()
            .any(|c| top > c.bottom && bottom < c.top)
    }
    fn local_hm(ns: u64) -> (u32, u32) {
        use chrono::Timelike;
        let dt = chrono::DateTime::from_timestamp_nanos(ns as i64)
            .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"));
        (dt.hour(), dt.minute())
    }

    pub fn update_confirmed(
        &mut self,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        bar_close_ns: u64,
        bar_ns: u64,
    ) -> Result<Observation> {
        self.bar_index += 1;
        self.bars.push_back(PriceBar { high, low });
        let keep = self
            .settings
            .required_warmup()
            .max(self.settings.trail_lookback + 8);
        while self.bars.len() > keep {
            self.bars.pop_front();
        }
        let atr = self.update_atr(high, low, close);

        if let Some((hi, lo)) = self.range(self.settings.normalization_length)
            && hi > lo
        {
            self.normalized.push_back((close - lo) / (hi - lo));
            while self.normalized.len() > 32 {
                self.normalized.pop_front();
            }
            if let Some(v) = self.stdev14() {
                self.vol.push_back(v);
                while self.vol.len() > self.settings.box_detection_length + 2 {
                    self.vol.pop_front();
                }
            }
        }

        let mut bull = false;
        let mut bear = false;
        if self.vol.len() > self.settings.box_detection_length {
            let n = self.settings.box_detection_length + 1;
            let hb = self.extremum_back(true, n).unwrap();
            let lb = self.extremum_back(false, n).unwrap();
            let len = self.settings.box_detection_length as f64;
            let upper = (len - hb as f64) / len;
            let lower = (len - lb as f64) / len;
            let lower_cross = self
                .prev_lower
                .zip(self.prev_upper)
                .is_some_and(|(pl, pu)| pl <= pu && lower > upper);
            let upper_cross = self
                .prev_upper
                .zip(self.prev_lower)
                .is_some_and(|(pu, pl)| pu <= pl && upper > lower);
            if lower_cross {
                self.since_lower_cross = 1;
            } else {
                self.since_lower_cross = self.since_lower_cross.saturating_add(1);
            }
            if upper_cross && self.since_lower_cross > 10 {
                let d = self.since_lower_cross.min(self.bars.len());
                if let Some((h, l)) = self.range(d)
                    && (self.settings.overlap || self.can_create(h, l))
                {
                    self.channels.insert(0, Channel { top: h, bottom: l });
                }
            }
            self.prev_upper = Some(upper);
            self.prev_lower = Some(lower);
        }

        let test = if self.settings.strong_closes_only {
            (close + open) / 2.0
        } else {
            close
        };
        for i in (0..self.channels.len()).rev() {
            if test > self.channels[i].top {
                self.channels.remove(i);
                bull = true;
            } else if test < self.channels[i].bottom {
                self.channels.remove(i);
                bear = true;
            }
        }
        let (channel_top, channel_bottom) = self
            .channels
            .first()
            .map_or((None, None), |c| (Some(c.top), Some(c.bottom)));
        let wick = self.range(self.settings.wick_lookback);
        let trail = self.range(self.settings.trail_lookback);
        let bar_open_ns = bar_close_ns.saturating_sub(bar_ns);
        let in_session = self
            .settings
            .session
            .contains(bar_open_ns, &self.calendar)?;
        let (h, m) = Self::local_hm(bar_close_ns);
        let eod = self.settings.force_flat_at_session_end
            && (h * 60 + m
                >= self.settings.auto_sq_off_hour * 60 + self.settings.auto_sq_off_minute);
        let can_enter = (!self.settings.allow_entries_only_in_session || in_session) && !eod;
        Ok(Observation {
            bar_close_ns,
            confirmed: true,
            close,
            atr,
            wick_low: wick.map(|x| x.1),
            wick_high: wick.map(|x| x.0),
            trail_low: trail.map(|x| x.1),
            trail_high: trail.map(|x| x.0),
            channel_top,
            channel_bottom,
            channel_count: self.channels.len(),
            bullish_breakout: bull && !bear,
            bearish_breakout: bear && !bull,
            in_session,
            eod_hit: eod,
            can_enter,
        })
    }

    pub fn preview_live(&self, close: f64, bar_close_ns: u64) -> Observation {
        Observation {
            bar_close_ns,
            confirmed: false,
            close,
            atr: self.atr,
            wick_low: None,
            wick_high: None,
            trail_low: None,
            trail_high: None,
            channel_top: self.channels.first().map(|c| c.top),
            channel_bottom: self.channels.first().map(|c| c.bottom),
            channel_count: self.channels.len(),
            bullish_breakout: false,
            bearish_breakout: false,
            in_session: false,
            eod_hit: false,
            can_enter: false,
        }
    }
}

//! Pine sections 6.2 (adaptive multipliers) and 6.3 (asymmetric SuperTrend with
//! band ratchet and character-flip detection).
use crate::indicators::{History, highest};
use crate::params::Params;
use serde::{Deserialize, Serialize};

const MULT_SMOOTH_ALPHA: f64 = 0.15;
const TQI_MULT_FLOOR: f64 = 0.6;
const TQI_MULT_RANGE: f64 = 0.8;
const ASYM_TIGHTEN_MAX: f64 = 0.3;
const ASYM_WIDEN_MAX: f64 = 0.4;

/// Per-bar inputs, all already computed by the engine.
pub struct StInputs {
    pub bar_index: i64,
    pub src: f64,
    pub close: f64,
    pub close_prev: Option<f64>,
    pub close_window: Option<f64>,
    pub atr_value: Option<f64>,
    pub er: f64,
    pub tqi: f64,
    pub eff_quality: f64,
    pub base_mult: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StOutput {
    pub trend: i8,
    pub prev_trend: i8,
    pub lower_band: Option<f64>,
    pub upper_band: Option<f64>,
    pub lower_band_prev: Option<f64>,
    pub upper_band_prev: Option<f64>,
    pub price_flip_up: bool,
    pub price_flip_down: bool,
    pub char_flip_up: bool,
    pub char_flip_down: bool,
    /// `flipUp` / `flipDown`: the trend changed on this bar.
    pub flip_up: bool,
    pub flip_down: bool,
    pub active_mult: f64,
    pub passive_mult: f64,
}

impl StOutput {
    pub fn line(&self) -> Option<f64> {
        if self.trend == 1 { self.lower_band } else { self.upper_band }
    }
    /// Flip caused only by the quality collapse (diagnostic alert in Pine).
    pub fn char_flip_fired(&self) -> bool {
        (self.char_flip_up && !self.price_flip_up && self.flip_up)
            || (self.char_flip_down && !self.price_flip_down && self.flip_down)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AdaptiveSuperTrend {
    active_sm: Option<f64>,
    passive_sm: Option<f64>,
    lower: Option<f64>,
    upper: Option<f64>,
    trend: Option<i8>,
    trend_start_bar: i64,
    tqi_hist: History<f64>,
}

impl AdaptiveSuperTrend {
    pub fn new(p: &Params) -> Self {
        Self {
            active_sm: None,
            passive_sm: None,
            lower: None,
            upper: None,
            trend: None,
            trend_start_bar: 0,
            tqi_hist: History::new(p.char_flip_min_age.max(3) as usize),
        }
    }

    pub fn update(&mut self, p: &Params, i: &StInputs) -> StOutput {
        // ── 6.2 adaptive multipliers ──
        let legacy = if p.use_adaptive { 1.0 + p.adaptation_strength * (0.5 - i.er) } else { 1.0 };
        let deviation = if p.use_tqi { (1.0 - i.tqi).powf(p.quality_curve_power) } else { 0.5 };
        let q = i.eff_quality;
        let tqi_mult = 1.0 - q + q * (TQI_MULT_FLOOR + TQI_MULT_RANGE * deviation);
        let sym = i.base_mult * legacy * tqi_mult;
        let (active_raw, passive_raw) = if p.use_tqi && p.asymmetric_bands {
            (
                sym * (1.0 - p.asymmetry_strength * i.tqi * ASYM_TIGHTEN_MAX),
                sym * (1.0 + p.asymmetry_strength * i.tqi * ASYM_WIDEN_MAX),
            )
        } else {
            (sym, sym)
        };
        let smooth = |prev: Option<f64>, raw: f64| match prev {
            Some(prev) if p.smooth_multipliers => prev * (1.0 - MULT_SMOOTH_ALPHA) + raw * MULT_SMOOTH_ALPHA,
            _ => raw,
        };
        let active = smooth(self.active_sm, active_raw);
        let passive = smooth(self.passive_sm, passive_raw);
        self.active_sm = Some(active);
        self.passive_sm = Some(passive);

        // ── 6.3 bands with ratchet ──
        let prev_trend = self.trend.unwrap_or(1);
        let (lower_mult, upper_mult) = if prev_trend == 1 { (active, passive) } else { (passive, active) };
        let lower_raw = i.atr_value.map(|a| i.src - lower_mult * a);
        let upper_raw = i.atr_value.map(|a| i.src + upper_mult * a);
        let lower_prev = self.lower;
        let upper_prev = self.upper;
        let lower = match lower_prev {
            None => lower_raw,
            Some(lb) if i.close_prev.is_some_and(|c| c > lb) => lower_raw.map(|r| r.max(lb)),
            Some(_) => lower_raw,
        };
        let upper = match upper_prev {
            None => upper_raw,
            Some(ub) if i.close_prev.is_some_and(|c| c < ub) => upper_raw.map(|r| r.min(ub)),
            Some(_) => upper_raw,
        };
        self.lower = lower;
        self.upper = upper;

        let price_flip_up = prev_trend == -1 && upper_prev.is_some_and(|u| i.close > u);
        let price_flip_down = prev_trend == 1 && lower_prev.is_some_and(|l| i.close < l);

        let trend_age = i.bar_index - self.trend_start_bar;
        let window = p.char_flip_min_age.max(3) as usize;
        self.tqi_hist.push(i.tqi);
        let tqi_window_high = highest(&self.tqi_hist, window);
        let base = p.character_flip
            && p.use_tqi
            && trend_age >= p.char_flip_min_age
            && tqi_window_high.is_some_and(|h| h > p.char_flip_high_tqi)
            && i.tqi < p.char_flip_low_tqi;
        let char_flip_down = base && prev_trend == 1 && i.close_window.is_some_and(|c| i.close < c);
        let char_flip_up = base && prev_trend == -1 && i.close_window.is_some_and(|c| i.close > c);

        let previous = self.trend;
        let trend = match previous {
            None => 1,
            Some(_) if price_flip_up || char_flip_up => 1,
            Some(_) if price_flip_down || char_flip_down => -1,
            Some(_) => prev_trend,
        };
        self.trend = Some(trend);
        if trend != prev_trend {
            self.trend_start_bar = i.bar_index;
        }
        StOutput {
            trend,
            prev_trend,
            lower_band: lower,
            upper_band: upper,
            lower_band_prev: lower_prev,
            upper_band_prev: upper_prev,
            price_flip_up,
            price_flip_down,
            char_flip_up,
            char_flip_down,
            flip_up: trend == 1 && previous == Some(-1),
            flip_down: trend == -1 && previous == Some(1),
            active_mult: active,
            passive_mult: passive,
        }
    }
}

//! Inputs of Precision Sniper v2.1.0 that affect the trade model. Display / alert /
//! position-calculator inputs are not ported. HTF bias is not ported (the script's
//! default has it OFF: empty HTF timeframe). Session filter defaults to "Info only".
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Preset {
    Auto,
    Conservative,
    Default,
    Aggressive,
    Scalping,
    Swing,
    #[serde(rename = "Crypto 24/7")]
    Crypto,
    Custom,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum VolMode {
    Off,
    #[serde(rename = "Skip Signals")]
    Skip,
    #[serde(rename = "Widen SL")]
    Widen,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum GradeFilter {
    All,
    #[serde(rename = "A+ and A")]
    AOrBetter,
    #[serde(rename = "A+ Only")]
    APlus,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum StructurePolicy {
    #[serde(rename = "Cap and flag")]
    Cap,
    #[serde(rename = "Skip entry")]
    Skip,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Params {
    pub preset: Preset,
    /// Chart timeframe, minutes (drives preset "Auto": <=5m Scalping, <=1h Default,
    /// <4h Conservative, else Swing).
    pub timeframe_minutes: u32,
    pub vwap: bool,
    pub warmup_mult: usize,
    // Custom preset values
    pub ema_fast: usize,
    pub ema_slow: usize,
    pub ema_trend: usize,
    pub rsi_len: usize,
    pub atr_len: usize,
    pub min_score: u32,
    pub sl_mult: f64,
    // Entry engine
    pub grade_filter: GradeFilter,
    pub hide_c: bool,
    pub vol_mode: VolMode,
    pub vol_threshold: f64,
    pub vol_widen: f64,
    pub confirm_window: usize,
    pub max_extension: f64,
    // Risk / execution model
    pub tp1_r: f64,
    pub tp2_r: f64,
    pub tp3_r: f64,
    /// Fraction of the ORIGINAL position closed at TP1 / TP2 (script: "Close original
    /// position at TP1/TP2 (%)" / 100). 0 = whole-position model. Sum must be < 1.
    pub tp1_close_fraction: f64,
    pub tp2_close_fraction: f64,
    pub step_stop: bool,
    pub full_tp3: bool,
    pub structure: bool,
    pub swing_lookback: usize,
    pub structure_policy: StructurePolicy,
    pub min_risk_ticks: u32,
    pub tick_size: f64,
    pub positive_only: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            preset: Preset::Auto,
            timeframe_minutes: 5,
            vwap: true,
            warmup_mult: 3,
            ema_fast: 9,
            ema_slow: 21,
            ema_trend: 55,
            rsi_len: 13,
            atr_len: 14,
            min_score: 5,
            sl_mult: 1.5,
            grade_filter: GradeFilter::All,
            hide_c: true,
            vol_mode: VolMode::Skip,
            vol_threshold: 1.3,
            vol_widen: 1.5,
            confirm_window: 0,
            max_extension: 0.0,
            tp1_r: 1.0,
            tp2_r: 2.0,
            tp3_r: 3.0,
            tp1_close_fraction: 0.0,
            tp2_close_fraction: 0.0,
            step_stop: true,
            full_tp3: true,
            structure: true,
            swing_lookback: 10,
            structure_policy: StructurePolicy::Cap,
            min_risk_ticks: 2,
            tick_size: 1.0,
            positive_only: true,
        }
    }
}

/// Preset-resolved lengths: (fast, slow, trend, rsi, atr, min score /10, ATR stop mult).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Resolved {
    pub preset: Preset,
    pub fast: usize,
    pub slow: usize,
    pub trend: usize,
    pub rsi: usize,
    pub atr: usize,
    pub min_score: u32,
    pub sl_mult: f64,
}

impl Params {
    pub fn resolve(&self) -> Resolved {
        let preset = match self.preset {
            Preset::Auto => match self.timeframe_minutes * 60 {
                s if s <= 300 => Preset::Scalping,
                s if s <= 3600 => Preset::Default,
                s if s < 14400 => Preset::Conservative,
                _ => Preset::Swing,
            },
            p => p,
        };
        let (fast, slow, trend, rsi, atr, min_score, sl_mult) = match preset {
            Preset::Scalping => (5, 13, 34, 8, 10, 4, 0.8),
            Preset::Aggressive => (8, 18, 50, 11, 12, 3, 1.2),
            Preset::Default => (9, 21, 55, 13, 14, 5, 1.5),
            Preset::Conservative => (12, 26, 89, 14, 14, 7, 2.0),
            Preset::Swing => (13, 34, 89, 21, 20, 6, 2.5),
            Preset::Crypto => (9, 21, 55, 14, 20, 5, 2.0),
            _ => (self.ema_fast, self.ema_slow, self.ema_trend, self.rsi_len, self.atr_len, self.min_score, self.sl_mult),
        };
        Resolved { preset, fast, slow, trend, rsi, atr, min_score, sl_mult }
    }

    pub fn validate(&self) -> Result<(), String> {
        let r = self.resolve();
        if !(r.fast < r.slow && r.slow < r.trend) {
            return Err("EMA periods must satisfy Fast < Slow < Trend".into());
        }
        if !(self.tp1_r < self.tp2_r && self.tp2_r < self.tp3_r) {
            return Err("TP multipliers must satisfy TP1 < TP2 < TP3".into());
        }
        let (f1, f2) = (self.tp1_close_fraction, self.tp2_close_fraction);
        if !(f1 >= 0.0 && f2 >= 0.0 && f1 + f2 < 1.0) {
            return Err("tp1/tp2 close fractions must be >= 0 and sum to < 1".into());
        }
        if !(self.tick_size.is_finite() && self.tick_size > 0.0) {
            return Err("tick_size must be positive".into());
        }
        if !(2..=10).contains(&self.warmup_mult) || self.swing_lookback < 3 || self.timeframe_minutes == 0 {
            return Err("warmup_mult 2..10, swing_lookback >= 3, timeframe_minutes > 0".into());
        }
        Ok(())
    }

    /// Evidence ratio a signal needs (script: max of preset score, hide-C 0.5, grade filter).
    pub fn required_ratio(&self) -> f64 {
        let grade = match self.grade_filter {
            GradeFilter::APlus => 0.8,
            GradeFilter::AOrBetter => 0.65,
            GradeFilter::All => 0.0,
        };
        let hide_c: f64 = if self.hide_c { 0.5 } else { 0.0 };
        (self.resolve().min_score as f64 / 10.0).max(hide_c.max(grade))
    }
}

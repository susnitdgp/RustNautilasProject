//! SATS inputs (Pine section 3), preset resolution (3.5) and the script's own
//! `runtime.error` checks. Every functional input is here; visual, dashboard,
//! colour and alert-text inputs are presentation only and are omitted.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum Preset {
    #[default]
    Auto,
    Custom,
    Scalping,
    Default,
    Swing,
    #[serde(rename = "Crypto 24/7")]
    Crypto247,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Open,
    High,
    Low,
    #[default]
    Close,
    Hl2,
    Hlc3,
    Ohlc4,
    Hlcc4,
}

impl Source {
    pub fn of(self, o: f64, h: f64, l: f64, c: f64) -> f64 {
        match self {
            Self::Open => o,
            Self::High => h,
            Self::Low => l,
            Self::Close => c,
            Self::Hl2 => (h + l) / 2.0,
            Self::Hlc3 => (h + l + c) / 3.0,
            Self::Ohlc4 => (o + h + l + c) / 4.0,
            Self::Hlcc4 => (h + l + c + c) / 4.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum TpMode {
    #[default]
    Fixed,
    Dynamic,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum TqiVolMode {
    #[default]
    #[serde(rename = "ATR regime")]
    AtrRegime,
    #[serde(rename = "Volume activity")]
    VolumeActivity,
}

/// All functional SATS v1.13.1 inputs with the script's defaults.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Params {
    // ── Main ──
    pub preset: Preset,
    pub atr_length: usize,
    pub base_band_width: f64,
    pub source: Source,
    // ── Adaptive Engine (legacy ER) ──
    pub use_adaptive: bool,
    pub efficiency_window: usize,
    pub adaptation_strength: f64,
    pub atr_baseline_length: usize,
    // ── Trend Quality Engine ──
    pub use_tqi: bool,
    pub quality_influence: f64,
    pub quality_curve_power: f64,
    pub smooth_multipliers: bool,
    pub asymmetric_bands: bool,
    pub asymmetry_strength: f64,
    pub efficiency_weighted_atr: bool,
    pub character_flip: bool,
    pub char_flip_min_age: i64,
    pub char_flip_high_tqi: f64,
    pub char_flip_low_tqi: f64,
    pub weight_efficiency: f64,
    pub weight_vol_factor: f64,
    pub weight_structure: f64,
    pub weight_momentum: f64,
    pub structure_window: usize,
    pub momentum_window: usize,
    pub tqi_volatility_factor: TqiVolMode,
    // ── Score components (score only, not entry filters) ──
    pub score_use_structure: bool,
    pub pivot_strength: usize,
    pub score_use_rsi: bool,
    pub rsi_length: usize,
    pub rsi_overbought: f64,
    pub rsi_oversold: f64,
    pub rsi_memory_bars: usize,
    pub score_use_volume: bool,
    pub volume_z_window: usize,
    // ── Risk management ──
    pub sl_buffer_atr: f64,
    pub max_sl_distance_atr: f64,
    pub tp_mode: TpMode,
    pub tp1_r: f64,
    pub tp2_r: f64,
    pub tp3_r: f64,
    pub trade_timeout_bars: i64,
    // ── Dynamic TP ──
    pub dyn_tp_tqi_influence: f64,
    pub dyn_tp_vol_influence: f64,
    pub dyn_tp_min_scale: f64,
    pub dyn_tp_max_scale: f64,
    pub dyn_tp1_floor_r: f64,
    pub dyn_tp_ceiling_r: f64,
    // ── Self-learning (experimental) ──
    pub auto_calibration: bool,
    pub calibration_window: usize,
    pub calibration_bad_r: f64,
    pub calibration_good_r: f64,
    pub calibration_quality_step: f64,
    pub calibration_cooldown: usize,
    pub calibration_quality_floor: f64,
    pub calibration_quality_ceiling: f64,
    // ── Execution model ──
    pub commission_pct_per_fill: f64,
    pub slippage_ticks: u32,
    pub min_risk_ticks: u32,
    pub max_pivot_age_bars: i64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            preset: Preset::Auto,
            atr_length: 13,
            base_band_width: 2.0,
            source: Source::Close,
            use_adaptive: true,
            efficiency_window: 20,
            adaptation_strength: 0.5,
            atr_baseline_length: 100,
            use_tqi: true,
            quality_influence: 0.4,
            quality_curve_power: 1.5,
            smooth_multipliers: true,
            asymmetric_bands: true,
            asymmetry_strength: 0.5,
            efficiency_weighted_atr: true,
            character_flip: true,
            char_flip_min_age: 5,
            char_flip_high_tqi: 0.55,
            char_flip_low_tqi: 0.25,
            weight_efficiency: 0.35,
            weight_vol_factor: 0.20,
            weight_structure: 0.25,
            weight_momentum: 0.20,
            structure_window: 20,
            momentum_window: 10,
            tqi_volatility_factor: TqiVolMode::AtrRegime,
            score_use_structure: true,
            pivot_strength: 3,
            score_use_rsi: true,
            rsi_length: 14,
            rsi_overbought: 70.0,
            rsi_oversold: 30.0,
            rsi_memory_bars: 20,
            score_use_volume: true,
            volume_z_window: 20,
            sl_buffer_atr: 1.5,
            max_sl_distance_atr: 4.0,
            tp_mode: TpMode::Fixed,
            tp1_r: 1.0,
            tp2_r: 2.0,
            tp3_r: 3.0,
            trade_timeout_bars: 100,
            dyn_tp_tqi_influence: 0.6,
            dyn_tp_vol_influence: 0.4,
            dyn_tp_min_scale: 0.5,
            dyn_tp_max_scale: 2.0,
            dyn_tp1_floor_r: 0.5,
            dyn_tp_ceiling_r: 8.0,
            auto_calibration: false,
            calibration_window: 20,
            calibration_bad_r: 0.0,
            calibration_good_r: 0.7,
            calibration_quality_step: 0.05,
            calibration_cooldown: 5,
            calibration_quality_floor: 0.1,
            calibration_quality_ceiling: 0.9,
            commission_pct_per_fill: 0.0,
            slippage_ticks: 0,
            min_risk_ticks: 2,
            max_pivot_age_bars: 100,
        }
    }
}

/// Section 3.5: preset-dependent values and the sorted fixed TP R-multiples.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resolved {
    pub preset: Preset,
    pub atr_len: usize,
    pub base_mult: f64,
    pub er_len: usize,
    pub rsi_len: usize,
    pub sl_mult: f64,
    pub fixed_tp1_r: f64,
    pub fixed_tp2_r: f64,
    pub fixed_tp3_r: f64,
    pub warmup_bars: i64,
}

fn range<T: PartialOrd + Copy>(v: T, lo: T, hi: T, what: &str, e: &mut Vec<String>) {
    if !(v >= lo && v <= hi) {
        e.push(what.to_owned());
    }
}

impl Params {
    /// Pine input minval/maxval limits plus the script's `runtime.error` checks.
    pub fn validate(&self) -> Result<(), String> {
        let mut e = Vec::new();
        range(self.atr_length, 5, 100, "atr_length must be 5..100", &mut e);
        range(self.base_band_width, 0.5, 5.0, "base_band_width must be 0.5..5.0", &mut e);
        range(self.efficiency_window, 5, 100, "efficiency_window must be 5..100", &mut e);
        range(self.adaptation_strength, 0.0, 1.0, "adaptation_strength must be 0..1", &mut e);
        range(self.atr_baseline_length, 20, 500, "atr_baseline_length must be 20..500", &mut e);
        range(self.quality_influence, 0.0, 1.0, "quality_influence must be 0..1", &mut e);
        range(self.quality_curve_power, 1.0, 3.0, "quality_curve_power must be 1..3", &mut e);
        range(self.asymmetry_strength, 0.0, 1.0, "asymmetry_strength must be 0..1", &mut e);
        range(self.char_flip_min_age, 1, 50, "char_flip_min_age must be 1..50", &mut e);
        range(self.char_flip_high_tqi, 0.3, 0.9, "char_flip_high_tqi must be 0.3..0.9", &mut e);
        range(self.char_flip_low_tqi, 0.0, 0.5, "char_flip_low_tqi must be 0..0.5", &mut e);
        for (w, n) in [
            (self.weight_efficiency, "weight_efficiency"),
            (self.weight_vol_factor, "weight_vol_factor"),
            (self.weight_structure, "weight_structure"),
            (self.weight_momentum, "weight_momentum"),
        ] {
            range(w, 0.0, 1.0, &format!("{n} must be 0..1"), &mut e);
        }
        range(self.structure_window, 5, 100, "structure_window must be 5..100", &mut e);
        range(self.momentum_window, 3, 50, "momentum_window must be 3..50", &mut e);
        range(self.pivot_strength, 2, 10, "pivot_strength must be 2..10", &mut e);
        range(self.rsi_length, 5, 50, "rsi_length must be 5..50", &mut e);
        range(self.rsi_overbought, 55.0, 90.0, "rsi_overbought must be 55..90", &mut e);
        range(self.rsi_oversold, 10.0, 45.0, "rsi_oversold must be 10..45", &mut e);
        range(self.rsi_memory_bars, 3, 100, "rsi_memory_bars must be 3..100", &mut e);
        range(self.volume_z_window, 5, 100, "volume_z_window must be 5..100", &mut e);
        range(self.sl_buffer_atr, 0.3, 5.0, "sl_buffer_atr must be 0.3..5.0", &mut e);
        range(self.max_sl_distance_atr, 1.0, 15.0, "max_sl_distance_atr must be 1..15", &mut e);
        for (r, n) in [(self.tp1_r, "tp1_r"), (self.tp2_r, "tp2_r"), (self.tp3_r, "tp3_r")] {
            range(r, 0.5, 10.0, &format!("{n} must be 0.5..10"), &mut e);
        }
        range(self.trade_timeout_bars, 10, 500, "trade_timeout_bars must be 10..500", &mut e);
        range(self.dyn_tp_tqi_influence, 0.0, 1.0, "dyn_tp_tqi_influence must be 0..1", &mut e);
        range(self.dyn_tp_vol_influence, 0.0, 1.0, "dyn_tp_vol_influence must be 0..1", &mut e);
        range(self.dyn_tp_min_scale, 0.2, 1.0, "dyn_tp_min_scale must be 0.2..1", &mut e);
        range(self.dyn_tp_max_scale, 1.0, 4.0, "dyn_tp_max_scale must be 1..4", &mut e);
        range(self.dyn_tp1_floor_r, 0.2, 2.0, "dyn_tp1_floor_r must be 0.2..2", &mut e);
        range(self.dyn_tp_ceiling_r, 2.0, 20.0, "dyn_tp_ceiling_r must be 2..20", &mut e);
        range(self.calibration_window, 5, 100, "calibration_window must be 5..100", &mut e);
        range(self.calibration_bad_r, -2.0, 1.0, "calibration_bad_r must be -2..1", &mut e);
        range(self.calibration_good_r, 0.0, 5.0, "calibration_good_r must be 0..5", &mut e);
        range(self.calibration_quality_step, 0.01, 0.3, "calibration_quality_step must be 0.01..0.3", &mut e);
        range(self.calibration_cooldown, 1, 50, "calibration_cooldown must be 1..50", &mut e);
        range(self.calibration_quality_floor, 0.0, 1.0, "calibration_quality_floor must be 0..1", &mut e);
        range(self.calibration_quality_ceiling, 0.0, 1.0, "calibration_quality_ceiling must be 0..1", &mut e);
        range(self.commission_pct_per_fill, 0.0, 5.0, "commission_pct_per_fill must be 0..5", &mut e);
        range(self.slippage_ticks, 0, 1000, "slippage_ticks must be 0..1000", &mut e);
        range(self.min_risk_ticks, 1, 10_000, "min_risk_ticks must be 1..10000", &mut e);
        range(self.max_pivot_age_bars, 5, 5000, "max_pivot_age_bars must be 5..5000", &mut e);
        // The script's own runtime.error checks.
        if self.use_tqi
            && self.weight_efficiency + self.weight_vol_factor + self.weight_structure + self.weight_momentum <= 0.0
        {
            e.push("TQI requires at least one positive weight".into());
        }
        if self.character_flip && self.use_tqi && self.char_flip_low_tqi >= self.char_flip_high_tqi {
            e.push("Char-Flip Low must be below High".into());
        }
        if self.auto_calibration
            && (self.calibration_quality_floor >= self.calibration_quality_ceiling
                || self.calibration_bad_r >= self.calibration_good_r)
        {
            e.push("Calibration requires floor < ceiling and bad R < good R".into());
        }
        if e.is_empty() { Ok(()) } else { Err(e.join("; ")) }
    }

    /// Section 3.5 for a chart of `bar_minutes`, plus the section 6 warm-up length.
    pub fn resolve(&self, bar_minutes: f64) -> Resolved {
        let preset = match self.preset {
            Preset::Auto if bar_minutes <= 5.0 => Preset::Scalping,
            Preset::Auto if bar_minutes <= 240.0 => Preset::Default,
            Preset::Auto => Preset::Swing,
            p => p,
        };
        let (atr_len, base_mult, er_len, rsi_len, sl_mult) = match preset {
            Preset::Scalping => (10, 1.5, 14, 9, 1.0),
            Preset::Default => (14, 2.0, 20, 14, 1.5),
            Preset::Swing => (21, 2.5, 30, 21, 2.0),
            Preset::Crypto247 => (14, 2.8, 20, 14, 2.5),
            Preset::Custom | Preset::Auto => (
                self.atr_length,
                self.base_band_width,
                self.efficiency_window,
                self.rsi_length,
                self.sl_buffer_atr,
            ),
        };
        let t1 = self.tp1_r.min(self.tp2_r.min(self.tp3_r));
        let t3 = self.tp1_r.max(self.tp2_r.max(self.tp3_r));
        let t2 = self.tp1_r + self.tp2_r + self.tp3_r - t1 - t3;
        let warmup = 50_usize
            .max(atr_len + self.atr_baseline_length - 2)
            .max(rsi_len + self.rsi_memory_bars - 1)
            .max(er_len)
            .max(self.momentum_window)
            .max(self.structure_window);
        Resolved {
            preset,
            atr_len,
            base_mult,
            er_len,
            rsi_len,
            sl_mult,
            fixed_tp1_r: t1,
            fixed_tp2_r: t2,
            fixed_tp3_r: t3,
            warmup_bars: warmup as i64,
        }
    }
}

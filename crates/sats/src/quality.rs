//! Pine sections 5, 5.3, 6.1 and 6.4: helper maths, efficiency ratio,
//! Trend Quality Index, dynamic TP scale and the 0..100 signal score.
use crate::params::{Params, TqiVolMode};

pub const ER_LOW_THRESH: f64 = 0.25;
pub const ER_HIGH_THRESH: f64 = 0.50;

/// `safeDiv`: fallback when the denominator is 0 or either side is na.
pub fn safe_div(num: Option<f64>, den: Option<f64>, fallback: f64) -> f64 {
    match (num, den) {
        (Some(n), Some(d)) if d != 0.0 => n / d,
        _ => fallback,
    }
}

pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    lo.max(hi.min(v))
}

/// `mapClamp`: a na input maps to `out_lo` (safeDiv falls back to 0).
pub fn map_clamp(v: Option<f64>, in_lo: f64, in_hi: f64, out_lo: f64, out_hi: f64) -> f64 {
    let t = clamp(safe_div(v.map(|v| v - in_lo), Some(in_hi - in_lo), 0.0), 0.0, 1.0);
    out_lo + t * (out_hi - out_lo)
}

/// `mapClampInv`.
pub fn map_clamp_inv(v: Option<f64>, in_lo: f64, in_hi: f64, out_high: f64, out_low: f64) -> f64 {
    let t = clamp(safe_div(v.map(|v| v - in_lo), Some(in_hi - in_lo), 0.0), 0.0, 1.0);
    out_high - t * (out_high - out_low)
}

/// `calcEfficiencyRatio`: `|src - src[n]| / sum(|src - src[1]|, n)`, 0 when na.
/// `src_hist` is newest-first and includes the current bar.
pub fn efficiency_ratio(src_hist: &crate::indicators::History<f64>, n: usize) -> f64 {
    if src_hist.len() < n + 1 {
        return 0.0;
    }
    let (Some(now), Some(then)) = (src_hist.get(0), src_hist.get(n)) else {
        return 0.0;
    };
    let vol: f64 = (0..n)
        .map(|k| (src_hist.get(k).unwrap_or(0.0) - src_hist.get(k + 1).unwrap_or(0.0)).abs())
        .sum();
    safe_div(Some((now - then).abs()), Some(vol), 0.0)
}

/// Inputs for one bar's TQI.
pub struct TqiInputs {
    pub er: f64,
    pub vol_ratio: f64,
    pub has_volume: bool,
    pub vol_z_raw: f64,
    pub struct_hi: Option<f64>,
    pub struct_lo: Option<f64>,
    pub close: f64,
    pub window_change: Option<f64>,
    pub up_moves: f64,
    pub down_moves: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tqi {
    pub er: f64,
    pub vol: f64,
    pub vol_available: bool,
    pub structure: f64,
    pub momentum: f64,
    pub weight_sum: f64,
    pub value: f64,
}

/// Section 6.1. `value` is 0.5 when the engine is disabled.
pub fn tqi(p: &Params, i: &TqiInputs) -> Tqi {
    let er = clamp(i.er, 0.0, 1.0);
    let vol_available = p.tqi_volatility_factor == TqiVolMode::AtrRegime || i.has_volume;
    let vol = match p.tqi_volatility_factor {
        TqiVolMode::AtrRegime => map_clamp(Some(i.vol_ratio), 0.6, 1.8, 0.0, 1.0),
        TqiVolMode::VolumeActivity if i.has_volume => map_clamp(Some(i.vol_z_raw), -1.0, 2.0, 0.0, 1.0),
        TqiVolMode::VolumeActivity => 0.0,
    };
    let range = match (i.struct_hi, i.struct_lo) {
        (Some(h), Some(l)) => Some(h - l),
        _ => None,
    };
    let price_pos = safe_div(i.struct_lo.map(|l| i.close - l), range, 0.5);
    let structure = clamp((price_pos - 0.5).abs() * 2.0, 0.0, 1.0);
    let n = p.momentum_window as f64;
    let momentum = match i.window_change {
        Some(c) if c > 0.0 => i.up_moves / n,
        Some(c) if c < 0.0 => i.down_moves / n,
        _ => 0.0,
    };
    let vol_w = if vol_available { p.weight_vol_factor } else { 0.0 };
    let weight_sum = p.weight_efficiency + vol_w + p.weight_structure + p.weight_momentum;
    let denom = if weight_sum > 0.0 { weight_sum } else { 1.0 };
    let raw = if p.use_tqi {
        (er * p.weight_efficiency + vol * vol_w + structure * p.weight_structure + momentum * p.weight_momentum) / denom
    } else {
        0.5
    };
    Tqi { er, vol, vol_available, structure, momentum, weight_sum, value: clamp(raw, 0.0, 1.0) }
}

/// Section 5.3 `calcDynTpScale`.
pub fn dyn_tp_scale(p: &Params, tqi: f64, vol_ratio: f64) -> f64 {
    let tqi_c = clamp(tqi, 0.0, 1.0);
    let vol_c = clamp(map_clamp(Some(vol_ratio), 0.5, 2.0, 0.0, 1.0), 0.0, 1.0);
    let w = p.dyn_tp_tqi_influence + p.dyn_tp_vol_influence;
    if w <= 0.0 {
        return 1.0;
    }
    let raw = (tqi_c * p.dyn_tp_tqi_influence + vol_c * p.dyn_tp_vol_influence) / w;
    p.dyn_tp_min_scale + raw * (p.dyn_tp_max_scale - p.dyn_tp_min_scale)
}

/// Inputs for `calcScoreBreakdown`.
pub struct ScoreInputs {
    pub is_buy: bool,
    pub close: f64,
    pub close_3: Option<f64>,
    pub atr_value: Option<f64>,
    pub er: f64,
    pub has_volume: bool,
    pub vol_z: f64,
    pub rsi_lo: Option<f64>,
    pub rsi_hi: Option<f64>,
    pub last_pivot_low: Option<f64>,
    pub last_pivot_high: Option<f64>,
    pub valid_low_pivot: bool,
    pub valid_high_pivot: bool,
    pub upper_band_prev: Option<f64>,
    pub lower_band_prev: Option<f64>,
}

/// Section 6.4: total score on the 0..100 scale (normalised by the available maximum).
pub fn score(p: &Params, i: &ScoreInputs) -> (f64, f64) {
    let dir_move = i
        .close_3
        .map(|c3| if i.is_buy { i.close - c3 } else { c3 - i.close });
    let mom = map_clamp(Some(safe_div(dir_move, i.atr_value, 0.0)), 0.3, 2.0, 0.0, 17.0);
    let er = map_clamp(Some(i.er), 0.15, 0.7, 0.0, 17.0);
    let vol = if p.score_use_volume && i.has_volume { map_clamp(Some(i.vol_z), 0.0, 3.0, 0.0, 17.0) } else { 0.0 };
    let depth = if i.is_buy {
        i.rsi_lo.map(|lo| (p.rsi_oversold - lo).max(0.0))
    } else {
        i.rsi_hi.map(|hi| (hi - p.rsi_overbought).max(0.0))
    };
    let rsi = if p.score_use_rsi { map_clamp(depth, 0.0, 15.0, 0.0, 17.0) } else { 0.0 };
    let piv_dist = if i.is_buy {
        i.last_pivot_low.map_or(0.0, |pl| (i.close - pl).abs())
    } else {
        i.last_pivot_high.map_or(0.0, |ph| (ph - i.close).abs())
    };
    let pivot_ok = if i.is_buy { i.valid_low_pivot } else { i.valid_high_pivot };
    let structure = if p.score_use_structure && pivot_ok {
        map_clamp_inv(Some(safe_div(Some(piv_dist), i.atr_value, 0.0)), 0.0, 1.5, 16.0, 6.0)
    } else {
        0.0
    };
    let break_depth = if i.is_buy {
        i.upper_band_prev.map(|u| (i.close - u).max(0.0))
    } else {
        i.lower_band_prev.map(|l| (l - i.close).max(0.0))
    };
    let brk = map_clamp(Some(safe_div(break_depth, i.atr_value, 0.0)), 0.0, 1.0, 0.0, 16.0);
    let available = 50.0
        + if p.score_use_volume && i.has_volume { 17.0 } else { 0.0 }
        + if p.score_use_rsi { 17.0 } else { 0.0 }
        + if p.score_use_structure && pivot_ok { 16.0 } else { 0.0 };
    (100.0 * (mom + er + vol + rsi + structure + brk) / available, available)
}

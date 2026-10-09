//! Script inputs (Pine SECTION 1) and preset resolution (SECTION 2).
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum Sensitivity {
    Conservative,
    #[default]
    Balanced,
    Aggressive,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum TouchMode {
    Full,
    Edge,
    Mid,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum ExitTarget {
    #[default]
    #[serde(rename = "TP1")]
    Tp1,
    #[serde(rename = "TP2")]
    Tp2,
    #[serde(rename = "TP3")]
    Tp3,
}

impl ExitTarget {
    pub const fn index(self) -> u8 {
        match self {
            Self::Tp1 => 1,
            Self::Tp2 => 2,
            Self::Tp3 => 3,
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tp1 => "TP1",
            Self::Tp2 => "TP2",
            Self::Tp3 => "TP3",
        }
    }
}

/// "Use Manual Overrides" group. When present it replaces the sensitivity preset
/// (touch mode is then "Edge", exactly as the Pine script does).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManualOverrides {
    pub comp_min_bars: usize,
    pub comp_max_bars: usize,
    pub atr_contraction: f64,
    pub extreme_zone: f64,
}

impl Default for ManualOverrides {
    fn default() -> Self {
        Self {
            comp_min_bars: 4,
            comp_max_bars: 14,
            atr_contraction: 0.82,
            extreme_zone: 0.35,
        }
    }
}

/// All functional inputs of VCE-Mojo v1.6 with the script's defaults.
/// Visual / dashboard inputs are not part of the engine.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Params {
    pub sensitivity: Sensitivity,
    pub catalyst_mode: bool,
    pub overrides: Option<ManualOverrides>,
    pub bg_atr_period: usize,
    pub session_lookback: usize,
    pub atr_period: usize,
    pub session_warmup_bars: u32,
    pub post_outcome_gap: u32,
    pub sl_buffer_atr: f64,
    pub sl_min_dist_atr: f64,
    pub sl_max_dist_atr: f64,
    pub tp1_r: f64,
    pub tp2_r: f64,
    pub tp3_r: f64,
    pub exit_target: ExitTarget,
    pub eod_square_off: bool,
    pub eod_hour: u32,
    pub eod_minute: u32,
    /// Exchange clock offset used for the trading day and the EOD cut-off (IST = 330).
    pub utc_offset_minutes: i32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            sensitivity: Sensitivity::Balanced,
            catalyst_mode: true,
            overrides: None,
            bg_atr_period: 20,
            session_lookback: 50,
            atr_period: 14,
            session_warmup_bars: 15,
            post_outcome_gap: 3,
            sl_buffer_atr: 0.20,
            sl_min_dist_atr: 0.30,
            sl_max_dist_atr: 2.00,
            tp1_r: 1.0,
            tp2_r: 1.5,
            tp3_r: 2.0,
            exit_target: ExitTarget::Tp1,
            eod_square_off: true,
            eod_hour: 23,
            eod_minute: 15,
            utc_offset_minutes: 330,
        }
    }
}

/// Values after SECTION 2 parameter resolution plus the touch-mode constants
/// used by SECTIONS 5–7.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resolved {
    pub comp_min_bars: usize,
    pub comp_max_bars: usize,
    pub atr_contraction: f64,
    pub extreme_zone: f64,
    pub touch_mode: TouchMode,
    pub max_violations: u32,
    pub tol_mult: f64,
    pub watch_expiry: u64,
}

fn check(ok: bool, what: &str, errors: &mut Vec<String>) {
    if !ok {
        errors.push(what.to_owned());
    }
}

impl Params {
    /// Enforces the same minval/maxval limits as the Pine inputs.
    pub fn validate(&self) -> Result<(), String> {
        let mut e = Vec::new();
        if let Some(o) = &self.overrides {
            check(
                (3..=10).contains(&o.comp_min_bars),
                "comp_min_bars must be 3..10",
                &mut e,
            );
            check(
                (5..=20).contains(&o.comp_max_bars),
                "comp_max_bars must be 5..20",
                &mut e,
            );
            check(
                (0.50..=0.98).contains(&o.atr_contraction),
                "atr_contraction must be 0.50..0.98",
                &mut e,
            );
            check(
                (0.15..=0.50).contains(&o.extreme_zone),
                "extreme_zone must be 0.15..0.50",
                &mut e,
            );
        }
        check(
            (10..=50).contains(&self.bg_atr_period),
            "bg_atr_period must be 10..50",
            &mut e,
        );
        check(
            (20..=100).contains(&self.session_lookback),
            "session_lookback must be 20..100",
            &mut e,
        );
        check(
            (5..=30).contains(&self.atr_period),
            "atr_period must be 5..30",
            &mut e,
        );
        check(
            self.session_warmup_bars <= 60,
            "session_warmup_bars must be 0..60",
            &mut e,
        );
        check(
            self.post_outcome_gap <= 10,
            "post_outcome_gap must be 0..10",
            &mut e,
        );
        check(
            (0.10..=1.50).contains(&self.sl_buffer_atr),
            "sl_buffer_atr must be 0.10..1.50",
            &mut e,
        );
        check(
            (0.20..=2.00).contains(&self.sl_min_dist_atr),
            "sl_min_dist_atr must be 0.20..2.00",
            &mut e,
        );
        check(
            (1.00..=5.00).contains(&self.sl_max_dist_atr),
            "sl_max_dist_atr must be 1.00..5.00",
            &mut e,
        );
        check(
            (0.50..=3.00).contains(&self.tp1_r),
            "tp1_r must be 0.50..3.00",
            &mut e,
        );
        check(
            (1.00..=5.00).contains(&self.tp2_r),
            "tp2_r must be 1.00..5.00",
            &mut e,
        );
        check(
            (1.50..=8.00).contains(&self.tp3_r),
            "tp3_r must be 1.50..8.00",
            &mut e,
        );
        check(self.eod_hour <= 23, "eod_hour must be 0..23", &mut e);
        check(self.eod_minute <= 59, "eod_minute must be 0..59", &mut e);
        check(
            (-720..=840).contains(&self.utc_offset_minutes),
            "utc_offset_minutes out of range",
            &mut e,
        );
        if e.is_empty() {
            Ok(())
        } else {
            Err(e.join("; "))
        }
    }

    /// SECTION 2: preset or manual values, then touch-mode derived constants.
    pub fn resolve(&self) -> Resolved {
        let (min, max, contraction, zone, touch) = match (self.overrides, self.sensitivity) {
            (Some(o), _) => (
                o.comp_min_bars,
                o.comp_max_bars,
                o.atr_contraction,
                o.extreme_zone,
                TouchMode::Edge,
            ),
            (None, Sensitivity::Conservative) => (5, 12, 0.70, 0.25, TouchMode::Full),
            (None, Sensitivity::Balanced) => (4, 14, 0.82, 0.35, TouchMode::Edge),
            (None, Sensitivity::Aggressive) => (3, 16, 0.92, 0.45, TouchMode::Mid),
        };
        let (max_violations, tol_mult, watch_expiry) = match touch {
            TouchMode::Full => (1, 0.30, 5),
            TouchMode::Edge => (2, 0.50, 10),
            TouchMode::Mid => (3, 0.75, 15),
        };
        Resolved {
            comp_min_bars: min,
            comp_max_bars: max,
            atr_contraction: contraction,
            extreme_zone: zone,
            touch_mode: touch,
            max_violations,
            tol_mult,
            watch_expiry,
        }
    }

    pub fn exit_r(&self) -> f64 {
        match self.exit_target {
            ExitTarget::Tp1 => self.tp1_r,
            ExitTarget::Tp2 => self.tp2_r,
            ExitTarget::Tp3 => self.tp3_r,
        }
    }

    pub fn cutoff_minute(&self) -> u32 {
        self.eod_hour * 60 + self.eod_minute
    }
}

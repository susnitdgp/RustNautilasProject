//! Pine `LearnState.record` calibration path (experimental, off by default):
//! hill-climbing on Quality Influence over non-overlapping epochs of trades that
//! were entered under the current setting. Display-only statistics of the script
//! (rolling buffer, regime grid, streaks) are not part of the port.
use crate::params::Params;
use crate::quality::clamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Learn {
    /// Effective Quality Influence used by the bands.
    pub eff_q: f64,
    pub epoch: u64,
    signals_since_calib: usize,
    epoch_sum: f64,
    last_shift_dir: i8,
    avg_r_at_last_shift: Option<f64>,
    pub status: String,
}

impl Learn {
    /// `barstate.isfirst` initialisation.
    pub fn new(p: &Params) -> Self {
        let eff_q = if p.auto_calibration {
            clamp(p.quality_influence, p.calibration_quality_floor, p.calibration_quality_ceiling)
        } else {
            p.quality_influence
        };
        Self {
            eff_q,
            epoch: 0,
            signals_since_calib: 0,
            epoch_sum: 0.0,
            last_shift_dir: 0,
            avg_r_at_last_shift: None,
            status: "Collecting epoch".into(),
        }
    }

    /// Per-bar reset when calibration is off.
    pub fn on_bar(&mut self, p: &Params) {
        if !p.auto_calibration {
            self.eff_q = p.quality_influence;
        }
    }

    /// Called with a closed trade's net R and the epoch it was entered in.
    pub fn record(&mut self, p: &Params, r: f64, entry_epoch: u64) {
        if !(p.auto_calibration && p.use_tqi && entry_epoch == self.epoch) {
            return;
        }
        self.signals_since_calib += 1;
        self.epoch_sum += r;
        let required = p.calibration_window.max(p.calibration_cooldown);
        if self.signals_since_calib < required {
            return;
        }
        let post = self.epoch_sum / self.signals_since_calib as f64;
        if post < p.calibration_bad_r {
            let mut dir = if self.last_shift_dir == 0 {
                1
            } else if self.avg_r_at_last_shift.is_none_or(|prev| post >= prev) {
                self.last_shift_dir
            } else {
                -self.last_shift_dir
            };
            if (dir > 0 && self.eff_q >= p.calibration_quality_ceiling)
                || (dir < 0 && self.eff_q <= p.calibration_quality_floor)
            {
                dir = -dir;
            }
            let next = clamp(
                self.eff_q + f64::from(dir) * p.calibration_quality_step,
                p.calibration_quality_floor,
                p.calibration_quality_ceiling,
            );
            self.status = format!("{:.2} -> {:.2} (poor epoch)", self.eff_q, next);
            self.eff_q = next;
            self.last_shift_dir = dir;
        } else {
            self.status = if post > p.calibration_good_r { "Good epoch: hold Q" } else { "Neutral epoch: hold Q" }.into();
            if post > p.calibration_good_r {
                self.last_shift_dir = 0;
            }
        }
        self.avg_r_at_last_shift = Some(post);
        self.signals_since_calib = 0;
        self.epoch_sum = 0.0;
        self.epoch += 1;
    }
}

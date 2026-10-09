//! Pine built-ins the script relies on, reproduced with TradingView's semantics.
//!
//! `None` plays the role of Pine's `na`: these return `None` until enough bars
//! exist, and any comparison against `None` is false, as in Pine.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// `ta.rma(src, length)`: `na` for the first `length - 1` bars, seeded with the
/// SMA of the first `length` values, then `alpha = 1 / length` smoothing.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Rma {
    length: usize,
    seed_sum: f64,
    seed_count: usize,
    value: Option<f64>,
}

impl Rma {
    pub fn new(length: usize) -> Self {
        assert!(length > 0, "RMA length must be positive");
        Self {
            length,
            seed_sum: 0.0,
            seed_count: 0,
            value: None,
        }
    }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        match self.value {
            Some(prev) => {
                let alpha = 1.0 / self.length as f64;
                self.value = Some(alpha * x + (1.0 - alpha) * prev);
            }
            None => {
                self.seed_sum += x;
                self.seed_count += 1;
                if self.seed_count == self.length {
                    self.value = Some(self.seed_sum / self.length as f64);
                }
            }
        }
        self.value
    }
    pub fn value(&self) -> Option<f64> {
        self.value
    }
}

/// `ta.atr(length)` = `ta.rma(ta.tr(true), length)`. The first bar's true range
/// is `high - low` because there is no previous close.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Atr {
    rma: Rma,
    prev_close: Option<f64>,
}

impl Atr {
    pub fn new(length: usize) -> Self {
        Self {
            rma: Rma::new(length),
            prev_close: None,
        }
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let tr = true_range(high, low, self.prev_close);
        self.prev_close = Some(close);
        self.rma.update(tr)
    }
    pub fn value(&self) -> Option<f64> {
        self.rma.value()
    }
}

pub fn true_range(high: f64, low: f64, prev_close: Option<f64>) -> f64 {
    match prev_close {
        None => high - low,
        Some(pc) => (high - low).max((high - pc).abs()).max((low - pc).abs()),
    }
}

/// `ta.percentile_linear_interpolation(src, length, 50)`, which for the 50th
/// percentile is the median of the last `length` values. `na` until `length` values exist.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RollingMedian {
    length: usize,
    window: VecDeque<f64>,
}

impl RollingMedian {
    pub fn new(length: usize) -> Self {
        assert!(length > 0, "median length must be positive");
        Self {
            length,
            window: VecDeque::with_capacity(length + 1),
        }
    }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        self.window.push_back(x);
        if self.window.len() > self.length {
            self.window.pop_front();
        }
        if self.window.len() < self.length {
            return None;
        }
        let mut sorted: Vec<f64> = self.window.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        let rank = 0.5 * (self.length - 1) as f64;
        let lo = rank.floor() as usize;
        let hi = rank.ceil() as usize;
        Some(sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64))
    }
}

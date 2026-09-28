//! LazyBear Squeeze Momentum math used by MCX Crude PURE Squeeze Momentum v2.28.3.
use anyhow::{Result, ensure};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub bb_length: usize,
    pub kc_length: usize,
    pub kc_mult: f64,
    pub use_true_range: bool,
}

impl Config {
    pub fn validate(self) -> Result<()> {
        ensure!(self.bb_length > 0, "SQZ BB length must be positive");
        ensure!(self.kc_length > 0, "SQZ KC length must be positive");
        ensure!(
            self.kc_mult.is_finite() && self.kc_mult > 0.0,
            "SQZ KC multiplier must be positive"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Values {
    pub ready: bool,
    pub value: f64,
    pub squeeze_on: bool,
    pub squeeze_off: bool,
    pub squeeze_no: bool,
    pub long_strength1: bool,
    pub long_strength2: bool,
    pub long_strength3: bool,
    pub short_strength1: bool,
    pub short_strength2: bool,
    pub short_strength3: bool,
    pub long_weak_bar: bool,
    pub short_weak_bar: bool,
}

#[derive(Debug, Clone)]
pub struct Engine {
    cfg: Config,
    closes: VecDeque<f64>,
    highs: VecDeque<f64>,
    lows: VecDeque<f64>,
    ranges: VecDeque<f64>,
    raw_values: VecDeque<f64>,
    values: VecDeque<f64>,
}

impl Engine {
    pub fn new(cfg: Config) -> Result<Self> {
        cfg.validate()?;
        Ok(Self {
            cfg,
            closes: VecDeque::new(),
            highs: VecDeque::new(),
            lows: VecDeque::new(),
            ranges: VecDeque::new(),
            raw_values: VecDeque::new(),
            values: VecDeque::new(),
        })
    }

    pub fn preview(&self, high: f64, low: f64, close: f64) -> Values {
        self.calculate(high, low, close).unwrap_or_default()
    }

    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Values {
        let previous_close = self.closes.back().copied();
        let range = range_value(self.cfg.use_true_range, high, low, previous_close);
        let raw = self.raw_for(high, low, close);
        let values = self.calculate(high, low, close).unwrap_or_default();
        let keep = self.keep();
        push_limited(&mut self.closes, close, keep);
        push_limited(&mut self.highs, high, keep);
        push_limited(&mut self.lows, low, keep);
        push_limited(&mut self.ranges, range, keep);
        if let Some(raw) = raw {
            push_limited(&mut self.raw_values, raw, keep);
        }
        if values.ready {
            push_limited(&mut self.values, values.value, keep);
        }
        values
    }

    fn keep(&self) -> usize {
        self.cfg.bb_length.max(self.cfg.kc_length) * 3 + 10
    }

    fn raw_for(&self, high: f64, low: f64, close: f64) -> Option<f64> {
        let closes = window_with(&self.closes, close, self.cfg.kc_length)?;
        let highs = window_with(&self.highs, high, self.cfg.kc_length)?;
        let lows = window_with(&self.lows, low, self.cfg.kc_length)?;
        let highest = highs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let lowest = lows.iter().copied().fold(f64::INFINITY, f64::min);
        let mid = ((highest + lowest) / 2.0 + mean(&closes)) / 2.0;
        Some(close - mid)
    }

    fn calculate(&self, high: f64, low: f64, close: f64) -> Option<Values> {
        let bb_closes = window_with(&self.closes, close, self.cfg.bb_length)?;
        let kc_closes = window_with(&self.closes, close, self.cfg.kc_length)?;
        let current_range = range_value(
            self.cfg.use_true_range,
            high,
            low,
            self.closes.back().copied(),
        );
        let kc_ranges = window_with(&self.ranges, current_range, self.cfg.kc_length)?;

        let basis = mean(&bb_closes);
        // v2.28.3 intentionally uses KC MultFactor for BB deviation.
        let bb_dev = self.cfg.kc_mult * population_stdev(&bb_closes);
        let upper_bb = basis + bb_dev;
        let lower_bb = basis - bb_dev;
        let kc_ma = mean(&kc_closes);
        let range_ma = mean(&kc_ranges);
        let upper_kc = kc_ma + range_ma * self.cfg.kc_mult;
        let lower_kc = kc_ma - range_ma * self.cfg.kc_mult;
        let squeeze_on = lower_bb > lower_kc && upper_bb < upper_kc;
        let squeeze_off = lower_bb < lower_kc && upper_bb > upper_kc;
        let squeeze_no = !squeeze_on && !squeeze_off;

        let raw = self.raw_for(high, low, close)?;
        let raw_window = window_with(&self.raw_values, raw, self.cfg.kc_length)?;
        let value = linear_regression_endpoint(&raw_window);
        let p1 = self.values.back().copied().unwrap_or(value);
        let p2 = self.values.iter().rev().nth(1).copied();
        let p3 = self.values.iter().rev().nth(2).copied();
        Some(Values {
            ready: true,
            value,
            squeeze_on,
            squeeze_off,
            squeeze_no,
            long_strength1: value > 0.0 && value > p1,
            long_strength2: p2.is_some_and(|b| value > 0.0 && value > p1 && p1 > b),
            long_strength3: p2
                .zip(p3)
                .is_some_and(|(b, c)| value > 0.0 && value > p1 && p1 > b && b > c),
            short_strength1: value < 0.0 && value < p1,
            short_strength2: p2.is_some_and(|b| value < 0.0 && value < p1 && p1 < b),
            short_strength3: p2
                .zip(p3)
                .is_some_and(|(b, c)| value < 0.0 && value < p1 && p1 < b && b < c),
            long_weak_bar: value > 0.0 && value < p1,
            short_weak_bar: value < 0.0 && value > p1,
        })
    }
}

fn push_limited(values: &mut VecDeque<f64>, value: f64, keep: usize) {
    values.push_back(value);
    while values.len() > keep {
        values.pop_front();
    }
}

fn window_with(values: &VecDeque<f64>, current: f64, length: usize) -> Option<Vec<f64>> {
    if length == 0 || values.len() + 1 < length {
        return None;
    }
    let mut out = values
        .iter()
        .skip(values.len().saturating_sub(length - 1))
        .copied()
        .collect::<Vec<_>>();
    out.push(current);
    (out.len() == length).then_some(out)
}

fn range_value(use_true_range: bool, high: f64, low: f64, previous_close: Option<f64>) -> f64 {
    if !use_true_range {
        return high - low;
    }
    previous_close.map_or(high - low, |previous| {
        (high - low)
            .max((high - previous).abs())
            .max((low - previous).abs())
    })
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn population_stdev(values: &[f64]) -> f64 {
    let avg = mean(values);
    (values.iter().map(|v| (v - avg).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
}

fn linear_regression_endpoint(values: &[f64]) -> f64 {
    if values.len() <= 1 {
        return values.first().copied().unwrap_or(0.0);
    }
    let n = values.len() as f64;
    let mean_x = (n - 1.0) / 2.0;
    let mean_y = mean(values);
    let numerator = values
        .iter()
        .enumerate()
        .map(|(i, y)| (i as f64 - mean_x) * (y - mean_y))
        .sum::<f64>();
    let denominator = (0..values.len())
        .map(|i| (i as f64 - mean_x).powi(2))
        .sum::<f64>();
    let slope = if denominator > 0.0 {
        numerator / denominator
    } else {
        0.0
    };
    let intercept = mean_y - slope * mean_x;
    intercept + slope * (n - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_endpoint_matches_linear_series() {
        assert!((linear_regression_endpoint(&[1., 2., 3., 4., 5.]) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn engine_becomes_ready() {
        let mut engine = Engine::new(Config {
            bb_length: 3,
            kc_length: 3,
            kc_mult: 1.5,
            use_true_range: true,
        })
        .unwrap();
        for i in 0..8 {
            let close = 100.0 + i as f64;
            engine.update(close + 1.0, close - 1.0, close);
        }
        assert!(engine.preview(110.0, 108.0, 109.0).ready);
    }
}

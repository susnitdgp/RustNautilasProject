//! Small streaming indicators used by the engine.

/// Wilder ATR: simple mean of the first `length` true ranges, then RMA.
#[derive(Clone, Debug)]
pub struct Atr {
    length: usize,
    prev_close: Option<f64>,
    seed: Vec<f64>,
    value: Option<f64>,
}
impl Atr {
    pub fn new(length: usize) -> Self {
        Self { length, prev_close: None, seed: Vec::with_capacity(length), value: None }
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let tr = match self.prev_close {
            Some(pc) => (high - low).max((high - pc).abs()).max((low - pc).abs()),
            None => high - low,
        };
        self.prev_close = Some(close);
        self.value = match self.value {
            Some(v) => Some((v * (self.length as f64 - 1.0) + tr) / self.length as f64),
            None => {
                self.seed.push(tr);
                (self.seed.len() == self.length).then(|| self.seed.iter().sum::<f64>() / self.length as f64)
            }
        };
        self.value
    }
    pub fn value(&self) -> Option<f64> {
        self.value
    }
}

/// Session VWAP on typical price; reset at each session start.
#[derive(Clone, Debug, Default)]
pub struct Vwap {
    pv: f64,
    v: f64,
    last_tp: f64,
}
impl Vwap {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64, volume: f64) -> f64 {
        let tp = (high + low + close) / 3.0;
        self.last_tp = tp;
        if volume > 0.0 {
            self.pv += tp * volume;
            self.v += volume;
        }
        self.value()
    }
    pub fn value(&self) -> f64 {
        if self.v > 0.0 { self.pv / self.v } else { self.last_tp }
    }
}

/// Median of a slice (copies and sorts; windows are small).
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

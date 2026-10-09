//! Pine `ta.*` equivalents, streaming, with Pine's warm-up semantics
//! (EMA / RMA seeded with the SMA of the first `length` values; `None` until then).
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct Sma {
    len: usize,
    buf: VecDeque<f64>,
    sum: f64,
}
impl Sma {
    pub fn new(len: usize) -> Self {
        Self { len, buf: VecDeque::with_capacity(len + 1), sum: 0.0 }
    }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        self.buf.push_back(x);
        self.sum += x;
        if self.buf.len() > self.len {
            self.sum -= self.buf.pop_front().unwrap_or(0.0);
        }
        (self.buf.len() == self.len).then(|| self.sum / self.len as f64)
    }
}

/// Exponential average with Pine's SMA seed. `alpha` = 2/(n+1) (EMA) or 1/n (RMA).
#[derive(Clone, Debug)]
pub struct Smoothed {
    alpha: f64,
    seed: Sma,
    value: Option<f64>,
}
impl Smoothed {
    pub fn ema(len: usize) -> Self {
        Self { alpha: 2.0 / (len as f64 + 1.0), seed: Sma::new(len), value: None }
    }
    pub fn rma(len: usize) -> Self {
        Self { alpha: 1.0 / len as f64, seed: Sma::new(len), value: None }
    }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        self.value = match self.value {
            Some(prev) => Some(self.alpha * x + (1.0 - self.alpha) * prev),
            None => self.seed.update(x),
        };
        self.value
    }
    pub fn value(&self) -> Option<f64> {
        self.value
    }
}

/// `ta.rsi(src, len)`.
#[derive(Clone, Debug)]
pub struct Rsi {
    prev: Option<f64>,
    up: Smoothed,
    down: Smoothed,
}
impl Rsi {
    pub fn new(len: usize) -> Self {
        Self { prev: None, up: Smoothed::rma(len), down: Smoothed::rma(len) }
    }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        let prev = self.prev.replace(x)?;
        // update both averages every bar so they stay in step
        let u = self.up.update((x - prev).max(0.0));
        let d = self.down.update((prev - x).max(0.0));
        let (u, d) = (u?, d?);
        Some(if d == 0.0 { 100.0 } else if u == 0.0 { 0.0 } else { 100.0 - 100.0 / (1.0 + u / d) })
    }
}

/// `ta.macd(src, 12, 26, 9)` histogram.
#[derive(Clone, Debug)]
pub struct MacdHist {
    fast: Smoothed,
    slow: Smoothed,
    signal: Smoothed,
}
impl MacdHist {
    pub fn new(fast: usize, slow: usize, signal: usize) -> Self {
        Self { fast: Smoothed::ema(fast), slow: Smoothed::ema(slow), signal: Smoothed::ema(signal) }
    }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        let f = self.fast.update(x);
        let s = self.slow.update(x);
        let macd = f.zip(s).map(|(f, s)| f - s)?;
        let sig = self.signal.update(macd)?;
        Some(macd - sig)
    }
}

/// `ta.atr(len)`: RMA of true range (first bar: high - low).
#[derive(Clone, Debug)]
pub struct Atr {
    prev_close: Option<f64>,
    rma: Smoothed,
}
impl Atr {
    pub fn new(len: usize) -> Self {
        Self { prev_close: None, rma: Smoothed::rma(len) }
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let tr = match self.prev_close {
            Some(pc) => (high - low).max((high - pc).abs()).max((low - pc).abs()),
            None => high - low,
        };
        self.prev_close = Some(close);
        self.rma.update(tr)
    }
}

/// `ta.dmi(diLen, adxLen)` → (+DI, -DI, ADX).
#[derive(Clone, Debug)]
pub struct Dmi {
    prev: Option<(f64, f64, f64)>,
    tr: Smoothed,
    plus: Smoothed,
    minus: Smoothed,
    adx: Smoothed,
    last_plus: Option<f64>,
    last_minus: Option<f64>,
}
impl Dmi {
    pub fn new(di_len: usize, adx_len: usize) -> Self {
        Self {
            prev: None,
            tr: Smoothed::rma(di_len),
            plus: Smoothed::rma(di_len),
            minus: Smoothed::rma(di_len),
            adx: Smoothed::rma(adx_len),
            last_plus: None,
            last_minus: None,
        }
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<(f64, f64, f64)> {
        let (ph, pl, pc) = self.prev.replace((high, low, close))?;
        let up = high - ph;
        let down = pl - low;
        let plus_dm = if up > down && up > 0.0 { up } else { 0.0 };
        let minus_dm = if down > up && down > 0.0 { down } else { 0.0 };
        let tr = (high - low).max((high - pc).abs()).max((low - pc).abs());
        let trur = self.tr.update(tr);
        let p = self.plus.update(plus_dm);
        let m = self.minus.update(minus_dm);
        // fixnan: keep the last valid value when trur is 0
        if let (Some(t), Some(p), Some(m)) = (trur, p, m)
            && t != 0.0
        {
            self.last_plus = Some(100.0 * p / t);
            self.last_minus = Some(100.0 * m / t);
        }
        let (plus, minus) = (self.last_plus?, self.last_minus?);
        let sum = plus + minus;
        let adx = self.adx.update((plus - minus).abs() / if sum == 0.0 { 1.0 } else { sum })?;
        Some((plus, minus, 100.0 * adx))
    }
}

/// `ta.vwap(hlc3)` anchored to the session day (`day` key changes = reset).
#[derive(Clone, Debug, Default)]
pub struct Vwap {
    day: Option<i64>,
    pv: f64,
    v: f64,
}
impl Vwap {
    pub fn update(&mut self, day: i64, hlc3: f64, volume: f64) -> Option<f64> {
        if self.day != Some(day) {
            *self = Self { day: Some(day), ..Self::default() };
        }
        self.pv += hlc3 * volume;
        self.v += volume;
        (self.v > 0.0).then(|| self.pv / self.v)
    }
}

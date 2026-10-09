//! Pine built-ins used by SATS, with TradingView's na and warm-up behaviour.
//! `None` is Pine's `na`; any comparison with `None` is false.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// `ta.rma`: na input → na (state unchanged); seeded with the SMA of the first
/// `len` values; then `(prev * (len - 1) + x) / len`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Rma {
    len: usize,
    seed_sum: f64,
    seed_n: usize,
    value: Option<f64>,
}

impl Rma {
    pub fn new(len: usize) -> Self {
        assert!(len > 0, "RMA length must be positive");
        Self { len, seed_sum: 0.0, seed_n: 0, value: None }
    }
    pub fn update(&mut self, x: Option<f64>) -> Option<f64> {
        let x = x?;
        if self.len == 1 {
            self.value = Some(x);
            return self.value;
        }
        match self.value {
            Some(prev) => {
                let n = self.len as f64;
                self.value = Some((prev * (n - 1.0) + x) / n);
                self.value
            }
            None => {
                self.seed_sum += x;
                self.seed_n += 1;
                if self.seed_n == self.len {
                    self.value = Some(self.seed_sum / self.len as f64);
                }
                self.value
            }
        }
    }
}

/// `ta.atr(len)` = `ta.rma(ta.tr(true), len)`; the first bar's TR is `high - low`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Atr {
    rma: Rma,
    prev_close: Option<f64>,
}

impl Atr {
    pub fn new(len: usize) -> Self {
        Self { rma: Rma::new(len), prev_close: None }
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let tr = match self.prev_close {
            None => high - low,
            Some(pc) => (high - low).max((high - pc).abs()).max((low - pc).abs()),
        };
        self.prev_close = Some(close);
        self.rma.update(Some(tr))
    }
}

/// `ta.rsi(src, len)`: na on the first bar; a side at or below 1e-10 saturates
/// (down side tested first, so both dormant = 100).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Rsi {
    prev: Option<f64>,
    up: Rma,
    down: Rma,
}

impl Rsi {
    pub fn new(len: usize) -> Self {
        Self { prev: None, up: Rma::new(len), down: Rma::new(len) }
    }
    pub fn update(&mut self, src: f64) -> Option<f64> {
        let prev = self.prev.replace(src)?;
        let u = self.up.update(Some((src - prev).max(0.0)));
        let d = self.down.update(Some((prev - src).max(0.0)));
        let (u, d) = (u?, d?);
        Some(if d <= 1e-10 {
            100.0
        } else if u <= 1e-10 {
            0.0
        } else {
            100.0 - 100.0 / (1.0 + u / d)
        })
    }
}

/// Newest-first bar history; `get(k)` is Pine's `x[k]`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct History<T> {
    cap: usize,
    items: VecDeque<T>,
}

impl<T: Copy> History<T> {
    pub fn new(cap: usize) -> Self {
        Self { cap: cap.max(1), items: VecDeque::new() }
    }
    pub fn push(&mut self, v: T) {
        self.items.push_front(v);
        self.items.truncate(self.cap);
    }
    pub fn get(&self, k: usize) -> Option<T> {
        self.items.get(k).copied()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn last_n(&self, n: usize) -> impl Iterator<Item = T> + '_ {
        self.items.iter().take(n).copied()
    }
}

/// `math.sum(x, n)`: na until `n` bars exist or if any value in the window is na.
pub fn sum(h: &History<Option<f64>>, n: usize) -> Option<f64> {
    if h.len() < n {
        return None;
    }
    h.last_n(n).try_fold(0.0, |acc, v| v.map(|v| acc + v))
}

/// `ta.sma(x, n)` = `math.sum(x, n) / n`.
pub fn sma(h: &History<Option<f64>>, n: usize) -> Option<f64> {
    sum(h, n).map(|s| s / n as f64)
}

/// `ta.stdev(x, n)` (biased): `sqrt(max(0, E[x²] - E[x]²))`.
pub fn stdev(h: &History<Option<f64>>, n: usize) -> Option<f64> {
    let p = sum(h, n)?;
    let q = h.last_n(n).try_fold(0.0, |acc, v| v.map(|v| acc + v * v))?;
    let m = p / n as f64;
    Some((q / n as f64 - m * m).max(0.0).sqrt())
}

/// `ta.highest(x, n)`: na while fewer than `n` bars exist.
pub fn highest(h: &History<f64>, n: usize) -> Option<f64> {
    (h.len() >= n).then(|| h.last_n(n).fold(f64::NEG_INFINITY, f64::max))
}

/// `ta.lowest(x, n)`: na while fewer than `n` bars exist.
pub fn lowest(h: &History<f64>, n: usize) -> Option<f64> {
    (h.len() >= n).then(|| h.last_n(n).fold(f64::INFINITY, f64::min))
}

/// `ta.pivothigh(src, n, n)`: the value `n` bars back when it is the window's
/// maximum and no newer bar ties it (TradingView keeps the newest of equal
/// extremes), i.e. `>=` every older bar and `>` every newer bar.
pub fn pivot_high(h: &History<f64>, n: usize) -> Option<f64> {
    pivot(h, n, |c, x| c > x, |c, x| c >= x)
}

/// `ta.pivotlow(src, n, n)`: mirror of [`pivot_high`].
pub fn pivot_low(h: &History<f64>, n: usize) -> Option<f64> {
    pivot(h, n, |c, x| c < x, |c, x| c <= x)
}

fn pivot(h: &History<f64>, n: usize, newer: impl Fn(f64, f64) -> bool, older: impl Fn(f64, f64) -> bool) -> Option<f64> {
    if h.len() < 2 * n + 1 {
        return None;
    }
    let c = h.get(n)?;
    let newer_ok = (0..n).all(|k| h.get(k).is_some_and(|x| newer(c, x)));
    let older_ok = (n + 1..=2 * n).all(|k| h.get(k).is_some_and(|x| older(c, x)));
    (newer_ok && older_ok).then_some(c)
}

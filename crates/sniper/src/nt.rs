//! Sniper indicators on `nautilus-indicators` 0.64, used by the engine. Streaming wrappers
//! with the same API as the old [`crate::ta`] port (kept as a backup file): `None` until
//! the indicator is initialized.
//!
//! Differences from the Pine-exact `ta` port, all limited to the warm-up:
//! - EMA / Wilder averages seed with the first value, Pine seeds with the SMA of the first
//!   `length` values. The gap decays by (1 - alpha) per bar and is negligible after the
//!   engine's own warm-up (`trend EMA × warmup_mult` bars).
//! - RSI feeds a zero change on the first bar (Pine skips it) and is scaled 0..1, so it is
//!   multiplied by 100 here.
//! - ADX is not in the library: it is built from `DirectionalMovement` (+DM/-DM, Wilder),
//!   `AverageTrueRange` (Wilder) and a `WilderMovingAverage`, with Pine's `fixnan`.
use nautilus_indicators::{
    average::{
        MovingAverageType, ema::ExponentialMovingAverage, rma::WilderMovingAverage, sma::SimpleMovingAverage,
        vwap::VolumeWeightedAveragePrice,
    },
    indicator::{Indicator, MovingAverage},
    momentum::{dm::DirectionalMovement, macd::MovingAverageConvergenceDivergence, rsi::RelativeStrengthIndex},
    volatility::atr::AverageTrueRange,
};

const NANOS: f64 = 1_000_000_000.0;

/// `ta.ema(src, len)`.
#[derive(Debug)]
pub struct Ema(ExponentialMovingAverage);
impl Ema {
    pub fn new(len: usize) -> Self {
        Self(ExponentialMovingAverage::new(len, None))
    }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        self.0.update_raw(x);
        self.0.initialized().then(|| self.0.value())
    }
}

/// `ta.sma(src, len)`.
#[derive(Debug)]
pub struct Sma(SimpleMovingAverage);
impl Sma {
    pub fn new(len: usize) -> Self {
        Self(SimpleMovingAverage::new(len, None))
    }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        self.0.update_raw(x);
        self.0.initialized().then(|| self.0.value())
    }
}

/// `ta.rsi(src, len)` (Wilder averages), 0..100.
#[derive(Debug)]
pub struct Rsi(RelativeStrengthIndex);
impl Rsi {
    pub fn new(len: usize) -> Self {
        Self(RelativeStrengthIndex::new(len, Some(MovingAverageType::Wilder)))
    }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        self.0.update_raw(x);
        self.0.initialized().then_some(100.0 * self.0.value)
    }
}

/// `ta.macd(src, fast, slow, signal)` histogram: EMA MACD line minus its EMA signal.
#[derive(Debug)]
pub struct MacdHist {
    macd: MovingAverageConvergenceDivergence,
    signal: Ema,
}
impl MacdHist {
    pub fn new(fast: usize, slow: usize, signal: usize) -> Self {
        Self {
            macd: MovingAverageConvergenceDivergence::new(fast, slow, Some(MovingAverageType::Exponential), None),
            signal: Ema::new(signal),
        }
    }
    pub fn update(&mut self, x: f64) -> Option<f64> {
        self.macd.update_raw(x);
        if !self.macd.initialized() {
            return None;
        }
        let line = self.macd.value();
        Some(line - self.signal.update(line)?)
    }
}

/// `ta.atr(len)`: Wilder average of the true range (first bar: high - low).
#[derive(Debug)]
pub struct Atr(AverageTrueRange);
impl Atr {
    pub fn new(len: usize) -> Self {
        Self(AverageTrueRange::new(len, Some(MovingAverageType::Wilder), Some(true), None))
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        self.0.update_raw(high, low, close);
        self.0.initialized().then_some(self.0.value)
    }
}

/// `ta.dmi(diLen, adxLen)` → (+DI, -DI, ADX).
#[derive(Debug)]
pub struct Dmi {
    dm: DirectionalMovement,
    tr: AverageTrueRange,
    adx: WilderMovingAverage,
    last_plus: Option<f64>,
    last_minus: Option<f64>,
}
impl Dmi {
    pub fn new(di_len: usize, adx_len: usize) -> Self {
        Self {
            dm: DirectionalMovement::new(di_len, Some(MovingAverageType::Wilder)),
            tr: AverageTrueRange::new(di_len, Some(MovingAverageType::Wilder), Some(true), None),
            adx: WilderMovingAverage::new(adx_len, None),
            last_plus: None,
            last_minus: None,
        }
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<(f64, f64, f64)> {
        self.dm.update_raw(high, low);
        self.tr.update_raw(high, low, close);
        if !(self.dm.initialized() && self.tr.initialized()) {
            return None;
        }
        // fixnan: keep the last valid value when the true-range average is 0
        let t = self.tr.value;
        if t != 0.0 {
            self.last_plus = Some(100.0 * self.dm.pos / t);
            self.last_minus = Some(100.0 * self.dm.neg / t);
        }
        let (plus, minus) = (self.last_plus?, self.last_minus?);
        let sum = plus + minus;
        self.adx.update_raw((plus - minus).abs() / if sum == 0.0 { 1.0 } else { sum });
        self.adx.initialized().then(|| (plus, minus, 100.0 * self.adx.value()))
    }
}

/// `ta.vwap(hlc3)` anchored to the session day. The library resets on the UTC epoch day of
/// the timestamp, so the caller passes the bar time shifted to IST (`ist_secs`) to reset at
/// IST midnight like the `ta` backend. `None` until the day has traded volume.
#[derive(Debug)]
pub struct Vwap {
    inner: VolumeWeightedAveragePrice,
    day: Option<i64>,
    traded: bool,
}
impl Default for Vwap {
    fn default() -> Self {
        Self { inner: VolumeWeightedAveragePrice::new(), day: None, traded: false }
    }
}
impl Vwap {
    pub fn update(&mut self, ist_secs: i64, hlc3: f64, volume: f64) -> Option<f64> {
        let day = ist_secs.div_euclid(86_400);
        if self.day != Some(day) {
            self.day = Some(day);
            self.traded = false;
        }
        self.traded |= volume > 0.0;
        self.inner.update_raw(hlc3, volume, ist_secs as f64 * NANOS);
        self.traded.then_some(self.inner.value)
    }
}

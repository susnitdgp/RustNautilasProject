//! Native VWAP/EMA/MACD/ATR calculations; decisions use completed bars only.
use nautilus_indicators::{
    average::{MovingAverageType, ema::ExponentialMovingAverage, vwap::VolumeWeightedAveragePrice},
    indicator::{Indicator, MovingAverage},
    momentum::macd::MovingAverageConvergenceDivergence,
    volatility::atr::AverageTrueRange,
};
use serde::Serialize;
#[derive(Debug, Serialize)]
pub struct Reading {
    pub close: f64,
    pub vwap: f64,
    pub ema9: f64,
    pub ema21: f64,
    pub macd: f64,
    pub macd_signal: f64,
    pub atr: f64,
    pub cross: i8,
    pub entry: i8,
    pub ready: bool,
}
#[derive(Debug)]
pub struct Policy {
    fast: ExponentialMovingAverage,
    slow: ExponentialMovingAverage,
    macd: MovingAverageConvergenceDivergence,
    signal: ExponentialMovingAverage,
    atr: AverageTrueRange,
    vwap: VolumeWeightedAveragePrice,
    previous: Option<f64>,
    day: u64,
    volume: f64,
}
impl Policy {
    pub fn new() -> Self {
        Self {
            fast: ExponentialMovingAverage::new(9, None),
            slow: ExponentialMovingAverage::new(21, None),
            macd: MovingAverageConvergenceDivergence::new(
                12,
                26,
                Some(MovingAverageType::Exponential),
                None,
            ),
            signal: ExponentialMovingAverage::new(9, None),
            atr: AverageTrueRange::new(14, Some(MovingAverageType::Wilder), Some(true), None),
            vwap: VolumeWeightedAveragePrice::new(),
            previous: None,
            day: 0,
            volume: 0.,
        }
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64, volume: f64, ts: u64) -> Reading {
        let local_ts = ts + 19_800_000_000_000;
        let day = local_ts / 86_400_000_000_000;
        if day != self.day {
            self.vwap.reset();
            self.volume = 0.;
            self.day = day;
        }
        self.volume += volume;
        self.vwap
            .update_raw((high + low + close) / 3., volume, local_ts as f64);
        self.fast.update_raw(close);
        self.slow.update_raw(close);
        self.macd.update_raw(close);
        if self.macd.initialized {
            self.signal.update_raw(self.macd.value);
        }
        self.atr.update_raw(high, low, close);
        let diff = self.fast.value - self.slow.value;
        let cross = match self.previous {
            Some(p) if p <= 0. && diff > 0. => 1,
            Some(p) if p >= 0. && diff < 0. => -1,
            _ => 0,
        };
        self.previous = Some(diff);
        let ready = self.fast.initialized
            && self.slow.initialized
            && self.signal.initialized
            && self.atr.initialized
            && self.volume > 0.;
        let entry = confirm(
            cross,
            close,
            self.vwap.value,
            self.macd.value,
            self.signal.value,
            ready,
        );
        Reading {
            close,
            vwap: self.vwap.value,
            ema9: self.fast.value,
            ema21: self.slow.value,
            macd: self.macd.value,
            macd_signal: self.signal.value,
            atr: self.atr.value,
            cross,
            entry,
            ready,
        }
    }
}
fn confirm(cross: i8, close: f64, vwap: f64, macd: f64, signal: f64, ready: bool) -> i8 {
    if !ready {
        0
    } else if cross == 1 && close > vwap && macd > signal {
        1
    } else if cross == -1 && close < vwap && macd < signal {
        -1
    } else {
        0
    }
}
pub fn stop_price(entry: f64, atr: f64, long: bool) -> f64 {
    if long {
        (entry - 1.5 * atr).floor()
    } else {
        (entry + 1.5 * atr).ceil()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_fresh_cross_and_both_confirmations() {
        assert_eq!(confirm(1, 110., 100., 2., 1., true), 1);
        assert_eq!(confirm(-1, 90., 100., -2., -1., true), -1);
        for args in [
            (0, 110., 100., 2., 1., true),
            (1, 90., 100., 2., 1., true),
            (1, 110., 100., 0., 1., true),
            (1, 110., 100., 2., 1., false),
            (1, 100., 100., 2., 1., true),
        ] {
            assert_eq!(confirm(args.0, args.1, args.2, args.3, args.4, args.5), 0);
        }
        assert_eq!(stop_price(6000., 10.1, true), 5984.);
        assert_eq!(stop_price(6000., 10.1, false), 6016.);
    }
    #[test]
    fn resets_session_vwap_without_resetting_trend_warmup() {
        let mut p = Policy::new();
        for i in 0..100 {
            p.update(
                101.,
                99.,
                100.,
                10.,
                1_789_360_200_000_000_000 + i * 300_000_000_000,
            );
        }
        let r = p.update(202., 198., 200., 20., 1_789_446_600_000_000_000);
        assert_eq!(r.vwap, 200.);
        assert!(r.ready);
        let r = p.update(302., 298., 300., 20., 1_789_446_900_000_000_000);
        assert_eq!(r.vwap, 250.);
    }
}

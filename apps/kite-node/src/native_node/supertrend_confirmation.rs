//! Entry-only confirmation for Supertrend, using native MACD and session VWAP.
use nautilus_indicators::{
    average::{MovingAverageType, ema::ExponentialMovingAverage, vwap::VolumeWeightedAveragePrice},
    indicator::{Indicator, MovingAverage},
    momentum::macd::MovingAverageConvergenceDivergence,
};
#[derive(Debug)]
pub struct Confirmation {
    macd: MovingAverageConvergenceDivergence,
    signal: ExponentialMovingAverage,
    vwap: VolumeWeightedAveragePrice,
    day: u64,
    volume: f64,
}
impl Confirmation {
    pub fn new() -> Self {
        Self {
            macd: MovingAverageConvergenceDivergence::new(
                12,
                26,
                Some(MovingAverageType::Exponential),
                None,
            ),
            signal: ExponentialMovingAverage::new(9, None),
            vwap: VolumeWeightedAveragePrice::new(),
            day: 0,
            volume: 0.,
        }
    }
    pub fn update(
        &mut self,
        high: f64,
        low: f64,
        close: f64,
        volume: f64,
        ts: u64,
    ) -> (i8, serde_json::Value) {
        let local = ts + 19_800_000_000_000;
        let day = local / 86_400_000_000_000;
        if day != self.day {
            self.vwap.reset();
            self.volume = 0.;
            self.day = day;
        }
        self.volume += volume;
        self.vwap
            .update_raw((high + low + close) / 3., volume, local as f64);
        self.macd.update_raw(close);
        if self.macd.initialized {
            self.signal.update_raw(self.macd.value);
        }
        let ready = self.signal.initialized && self.volume > 0.;
        let direction = confirm(
            close,
            self.vwap.value,
            self.macd.value,
            self.signal.value,
            ready,
        );
        (
            direction,
            serde_json::json!({"vwap":self.vwap.value,"macd":self.macd.value,"macd_signal":self.signal.value,"ready":ready,"confirmation_direction":direction}),
        )
    }
}
fn confirm(close: f64, vwap: f64, macd: f64, signal: f64, ready: bool) -> i8 {
    if ready && close > vwap && macd > signal {
        1
    } else if ready && close < vwap && macd < signal {
        -1
    } else {
        0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directional_confirmation_is_strict_and_does_not_require_zero_line() {
        assert_eq!(confirm(110., 100., -1., -2., true), 1);
        assert_eq!(confirm(90., 100., 1., 2., true), -1);
        for args in [
            (100., 100., 2., 1., true),
            (110., 100., 1., 1., true),
            (110., 100., 2., 1., false),
            (90., 100., 2., 1., true),
        ] {
            assert_eq!(confirm(args.0, args.1, args.2, args.3, args.4), 0);
        }
    }
    #[test]
    fn session_reset_preserves_macd_warmup_and_blocks_zero_volume() {
        let mut c = Confirmation::new();
        for i in 0..100 {
            c.update(
                101.,
                99.,
                100.,
                10.,
                1_789_360_200_000_000_000 + i * 300_000_000_000,
            );
        }
        let (_, r) = c.update(202., 198., 200., 20., 1_789_446_600_000_000_000);
        assert_eq!(r["vwap"], 200.);
        assert_eq!(r["ready"], true);
        let (_, r) = c.update(302., 298., 300., 0., 1_789_533_000_000_000_000);
        assert_eq!(r["ready"], false);
    }
}

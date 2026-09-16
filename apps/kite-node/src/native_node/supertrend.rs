//! Supertrend bands with Nautilus ATR(7), Wilder smoothing, multiplier 2.
use nautilus_indicators::{average::MovingAverageType, volatility::atr::AverageTrueRange};
#[derive(Debug)]
pub struct Supertrend {
    pub atr: AverageTrueRange,
    upper: f64,
    lower: f64,
    close: Option<f64>,
    pub direction: Option<i8>,
}
impl Supertrend {
    pub fn new() -> Self {
        Self {
            atr: AverageTrueRange::new(7, Some(MovingAverageType::Wilder), Some(true), None),
            upper: 0.,
            lower: 0.,
            close: None,
            direction: None,
        }
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<(i8, f64)> {
        self.atr.update_raw(high, low, close);
        let prior_close = self.close.replace(close);
        if !self.atr.initialized {
            return None;
        }
        let basic_upper = (high + low) / 2. + 2. * self.atr.value;
        let basic_lower = (high + low) / 2. - 2. * self.atr.value;
        let Some(direction) = self.direction else {
            self.upper = basic_upper;
            self.lower = basic_lower;
            self.direction = Some(-1);
            return Some((-1, self.upper));
        };
        let previous = prior_close.expect("initialized indicator");
        self.upper = if basic_upper < self.upper || previous > self.upper {
            basic_upper
        } else {
            self.upper
        };
        self.lower = if basic_lower > self.lower || previous < self.lower {
            basic_lower
        } else {
            self.lower
        };
        let next = if direction == -1 {
            if close > self.upper { 1 } else { -1 }
        } else if close < self.lower {
            -1
        } else {
            1
        };
        self.direction = Some(next);
        Some((next, if next == 1 { self.lower } else { self.upper }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn warms_up_then_flips_both_directions_and_tracks_gap_true_range() {
        let mut s = Supertrend::new();
        for _ in 0..6 {
            assert_eq!(s.update(101., 99., 100.), None);
        }
        assert_eq!(s.update(101., 99., 100.), Some((-1, 104.)));
        assert_eq!(s.update(111., 109., 110.).unwrap().0, 1);
        assert!((s.atr.value - 23. / 7.).abs() < 1e-12);
        assert_eq!(s.update(91., 89., 90.).unwrap().0, -1);
    }
    #[test]
    fn equality_does_not_flip_and_prefix_is_causal() {
        let mut a = Supertrend::new();
        let mut b = Supertrend::new();
        for _ in 0..20 {
            assert_eq!(a.update(101., 99., 100.), b.update(101., 99., 100.));
        }
        assert_eq!(a.update(104., 100., 104.).unwrap().0, -1);
        assert_eq!(b.direction, Some(-1));
    }
}

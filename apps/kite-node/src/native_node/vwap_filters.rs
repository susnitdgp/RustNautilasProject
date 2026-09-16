//! Predeclared entry-only variants. No parameter search or higher-timeframe filter.
use super::vwap_signal::Reading;
use anyhow::{Result, bail};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Baseline,
    Trend,
    Breakout,
}
impl Variant {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "baseline" => Ok(Self::Baseline),
            "trend" => Ok(Self::Trend),
            "breakout" => Ok(Self::Breakout),
            _ => bail!("Variant must be baseline, trend or breakout"),
        }
    }
    pub fn entry_rule(self) -> &'static str {
        match self {
            Self::Baseline => {
                "Fresh EMA cross with close/VWAP and MACD/signal confirmation on same bar; next-open fill"
            }
            Self::Trend => {
                "Baseline plus strict MACD zero-line and both one-bar EMA slopes in trade direction; next-open fill"
            }
            Self::Breakout => {
                "Baseline setup followed within 3 bars by close beyond setup high/low with alignment retained; next-open fill"
            }
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Trend => "trend",
            Self::Breakout => "breakout",
        }
    }
}
#[derive(Debug)]
struct Setup {
    direction: i8,
    high: f64,
    low: f64,
    age: u8,
}
#[derive(Debug)]
pub struct Filter {
    variant: Variant,
    setup: Option<Setup>,
    day: u64,
}
impl Filter {
    pub fn new(variant: Variant) -> Self {
        Self {
            variant,
            setup: None,
            day: 0,
        }
    }
    pub fn apply(&mut self, r: &Reading, high: f64, low: f64, ts: u64) -> i8 {
        let day = (ts + 19_800_000_000_000) / 86_400_000_000_000;
        if day != self.day {
            self.setup = None;
            self.day = day;
        }
        match self.variant {
            Variant::Baseline => r.raw_entry,
            Variant::Trend => {
                let side = f64::from(r.raw_entry);
                if side * r.macd > 0. && side * r.ema9_slope > 0. && side * r.ema21_slope > 0. {
                    r.raw_entry
                } else {
                    0
                }
            }
            Variant::Breakout => {
                if let Some(mut setup) = self.setup.take() {
                    setup.age += 1;
                    let side = f64::from(setup.direction);
                    let aligned = r.ready
                        && side * (r.ema9 - r.ema21) > 0.
                        && side * (r.close - r.vwap) > 0.
                        && side * (r.macd - r.macd_signal) > 0.;
                    if aligned && setup.age <= 3 {
                        if (setup.direction == 1 && r.close > setup.high)
                            || (setup.direction == -1 && r.close < setup.low)
                        {
                            return setup.direction;
                        }
                        if setup.age < 3 {
                            self.setup = Some(setup);
                        }
                    }
                }
                if r.raw_entry != 0 {
                    self.setup = Some(Setup {
                        direction: r.raw_entry,
                        high,
                        low,
                        age: 0,
                    });
                }
                0
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn reading(side: i8) -> Reading {
        Reading {
            close: 110.,
            vwap: 100.,
            ema9: 105.,
            ema21: 101.,
            macd: 2.,
            macd_signal: 1.,
            atr: 5.,
            cross: side,
            raw_entry: side,
            entry: side,
            ready: true,
            ema9_slope: 1.,
            ema21_slope: 0.5,
        }
    }
    #[test]
    fn trend_requires_both_slopes_and_zero_line_in_both_directions() {
        let mut f = Filter::new(Variant::Trend);
        let mut r = reading(1);
        assert_eq!(f.apply(&r, 112., 108., 1), 1);
        r.macd = -1.;
        assert_eq!(f.apply(&r, 112., 108., 2), 0);
        r.macd = 2.;
        r.ema21_slope = 0.;
        assert_eq!(f.apply(&r, 112., 108., 3), 0);
        r.raw_entry = -1;
        r.macd = -2.;
        r.ema9_slope = -1.;
        r.ema21_slope = -0.5;
        assert_eq!(f.apply(&r, 112., 108., 4), -1);
    }
    #[test]
    fn breakout_waits_for_a_later_close_not_a_wick_and_consumes_setup() {
        let mut f = Filter::new(Variant::Breakout);
        let mut r = reading(1);
        assert_eq!(f.apply(&r, 112., 108., 1), 0);
        r.raw_entry = 0;
        r.close = 111.;
        assert_eq!(f.apply(&r, 120., 109., 2), 0);
        r.close = 113.;
        assert_eq!(f.apply(&r, 114., 110., 3), 1);
        assert_eq!(f.apply(&r, 114., 110., 4), 0);
    }
    #[test]
    fn short_breakout_requires_close_below_setup_low() {
        let mut f = Filter::new(Variant::Breakout);
        let mut r = reading(-1);
        r.close = 90.;
        r.ema9 = 95.;
        r.ema21 = 99.;
        r.macd = -2.;
        r.macd_signal = -1.;
        assert_eq!(f.apply(&r, 92., 88., 1), 0);
        r.raw_entry = 0;
        r.close = 88.;
        assert_eq!(f.apply(&r, 90., 80., 2), 0);
        r.close = 87.;
        assert_eq!(f.apply(&r, 90., 85., 3), -1);
    }
    #[test]
    fn breakout_expires_and_cancels_on_session_change_or_lost_confirmation() {
        let mut f = Filter::new(Variant::Breakout);
        let mut r = reading(1);
        f.apply(&r, 112., 108., 1);
        r.raw_entry = 0;
        for ts in 2..=4 {
            assert_eq!(f.apply(&r, 112., 108., ts), 0);
        }
        r.close = 120.;
        assert_eq!(f.apply(&r, 121., 110., 5), 0);
        r = reading(1);
        f.apply(&r, 112., 108., 6);
        r.raw_entry = 0;
        r.close = 120.;
        assert_eq!(f.apply(&r, 121., 110., 86_400_000_000_000), 0);
        r = reading(1);
        f.apply(&r, 112., 108., 86_400_000_000_001);
        r.raw_entry = 0;
        r.macd = 0.;
        assert_eq!(f.apply(&r, 121., 110., 86_400_000_000_002), 0);
        r.macd = 2.;
        r.close = 120.;
        assert_eq!(f.apply(&r, 121., 110., 86_400_000_000_003), 0);
    }
}

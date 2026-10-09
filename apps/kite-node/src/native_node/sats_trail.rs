//! Trailing stop for `exit_mode: "trail"` (one open position of a `sats` slot).
//!
//! * Before activation the stop is SATS's own SL.
//! * When price reaches TP1 the trail activates: the stop moves to breakeven
//!   (actual fill ± offset) and from then on also follows the SuperTrend line at
//!   each bar close. The tighter level is used; the stop never moves against
//!   the trade. There is no profit target: the trade ends on the stop, a trend
//!   flip, a timeout or the daily square-off.
//! * Live: [`Trail::on_price`] runs on every quote with the executable price
//!   (bid for a long, ask for a short). Backtest: [`Trail::on_bar`] replays one
//!   bar pessimistically.
use super::sats_config::TrailSettings;
use sats::Side;

#[derive(Clone, Debug, PartialEq)]
pub struct Trail {
    pub side: Side,
    /// Entry used for breakeven: the model entry until the real fill is known.
    pub entry: f64,
    pub stop: f64,
    pub tp1: f64,
    /// TP1 was reached: breakeven applied and SuperTrend trailing enabled.
    pub active: bool,
}

impl Trail {
    pub fn new(side: Side, entry: f64, sl: f64, tp1: f64) -> Self {
        Self { side, entry, stop: sl, tp1, active: false }
    }

    fn sign(&self) -> f64 {
        self.side.sign()
    }

    /// Moves the stop to `level` only if that tightens it. Returns true if it moved.
    fn tighten(&mut self, level: f64) -> bool {
        if self.sign() * (level - self.stop) > 0.0 {
            self.stop = level;
            true
        } else {
            false
        }
    }

    pub fn stage_label(&self) -> &'static str {
        if self.active { "trailing stop" } else { "initial SL" }
    }

    /// Activates on a favourable price at or beyond TP1; returns the move made.
    fn activate(&mut self, cfg: &TrailSettings, px: f64) -> Option<String> {
        if self.active || self.sign() * (px - self.tp1) < 0.0 {
            return None;
        }
        self.active = true;
        let be = self.entry + self.sign() * cfg.breakeven_offset_points;
        Some(if self.tighten(be) {
            format!("TP1 reached: stop -> breakeven {:.0}", self.stop)
        } else {
            format!("TP1 reached: trailing, stop stays {:.0}", self.stop)
        })
    }

    /// Live, every quote: `px` is the executable price (bid for a long, ask for a
    /// short). Returns the stop move (for the dashboard) and whether to exit now.
    pub fn on_price(&mut self, cfg: &TrailSettings, px: f64) -> (Option<String>, bool) {
        let moved = self.activate(cfg, px);
        (moved, self.sign() * (px - self.stop) <= 0.0)
    }

    /// At each bar close after activation: follow the SuperTrend line while the
    /// trend still agrees with the trade. Returns the new stop if it moved.
    pub fn on_bar_close(&mut self, cfg: &TrailSettings, trend: i8, line: Option<f64>) -> Option<f64> {
        let agrees = (trend == 1 && self.side == Side::Long) || (trend == -1 && self.side == Side::Short);
        if cfg.supertrend_trail && self.active && agrees && let Some(l) = line && self.tighten(l) {
            return Some(self.stop);
        }
        None
    }

    /// Backtest replay of one bar (no ticks), pessimistic: the stop in force at
    /// the open is checked first (a gap through it fills at the open); then TP1
    /// activation on the bar's favourable extreme; a breakeven stop raised inside
    /// the bar is assumed hit if the bar also reached it. Returns the exit price
    /// (before slippage) if stopped out.
    pub fn on_bar(&mut self, cfg: &TrailSettings, open: f64, high: f64, low: f64) -> Option<f64> {
        let d = self.sign();
        let (fav, adv) = if self.side == Side::Long { (high, low) } else { (low, high) };
        if d * (open - self.stop) <= 0.0 {
            return Some(open);
        }
        if d * (adv - self.stop) <= 0.0 {
            return Some(self.stop);
        }
        self.activate(cfg, fav);
        (d * (adv - self.stop) <= 0.0).then_some(self.stop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TrailSettings {
        TrailSettings::default()
    }

    #[test]
    fn long_breakeven_at_tp1_then_supertrend() {
        let c = cfg();
        let mut t = Trail::new(Side::Long, 8756.0, 8725.0, 8783.0);
        assert_eq!(t.on_price(&c, 8760.0), (None, false));
        // SuperTrend is ignored until TP1
        assert_eq!(t.on_bar_close(&c, 1, Some(8750.0)), None);
        let (moved, exit) = t.on_price(&c, 8783.0);
        assert_eq!(moved.as_deref(), Some("TP1 reached: stop -> breakeven 8756"));
        assert!(!exit && t.active);
        assert_eq!(t.on_bar_close(&c, 1, Some(8740.0)), None); // below the stop
        assert_eq!(t.on_bar_close(&c, 1, Some(8800.0)), Some(8800.0));
        assert_eq!(t.on_bar_close(&c, -1, Some(8820.0)), None); // down-trend line never moves a long
        assert_eq!(t.on_bar_close(&c, 1, Some(8790.0)), None); // never loosens
        // no profit target: far above TP1 it just keeps running
        assert_eq!(t.on_price(&c, 9000.0), (None, false));
        assert_eq!(t.on_price(&c, 8800.0), (None, true));
    }

    #[test]
    fn short_mirror_offset_and_initial_stop() {
        let c = TrailSettings { breakeven_offset_points: 2.0, ..cfg() };
        let mut t = Trail::new(Side::Short, 8758.0, 8790.0, 8730.0);
        assert_eq!(t.on_price(&c, 8790.0), (None, true)); // initial SL on the ask
        let mut t = Trail::new(Side::Short, 8758.0, 8790.0, 8730.0);
        t.on_price(&c, 8730.0);
        assert_eq!(t.stop, 8756.0); // breakeven 2 points in the short's favour
        assert_eq!(t.on_bar_close(&c, -1, Some(8745.0)), Some(8745.0));
        assert_eq!(t.on_bar_close(&c, -1, Some(8750.0)), None);
        assert_eq!(t.on_price(&c, 8745.0), (None, true));
    }

    #[test]
    fn supertrend_trail_can_be_disabled() {
        let c = TrailSettings { supertrend_trail: false, ..cfg() };
        let mut t = Trail::new(Side::Long, 100.0, 90.0, 110.0);
        t.on_price(&c, 110.0);
        assert_eq!(t.stop, 100.0);
        assert_eq!(t.on_bar_close(&c, 1, Some(108.0)), None);
    }

    #[test]
    fn backtest_bar_is_pessimistic() {
        let c = cfg();
        let mut t = Trail::new(Side::Long, 100.0, 90.0, 110.0);
        assert_eq!(t.on_bar(&c, 85.0, 95.0, 80.0), Some(85.0)); // gap through the stop
        let mut t = Trail::new(Side::Long, 100.0, 90.0, 110.0);
        assert_eq!(t.on_bar(&c, 101.0, 111.0, 99.0), Some(100.0)); // TP1 then back to entry
        let mut t = Trail::new(Side::Long, 100.0, 90.0, 110.0);
        assert_eq!(t.on_bar(&c, 101.0, 112.0, 101.0), None);
        assert!(t.active && t.stop == 100.0);
        assert_eq!(t.on_bar(&c, 120.0, 200.0, 119.0), None); // no target
        let mut s = Trail::new(Side::Short, 100.0, 110.0, 90.0);
        assert_eq!(s.on_bar(&c, 101.0, 111.0, 95.0), Some(110.0));
    }
}

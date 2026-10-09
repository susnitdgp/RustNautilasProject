//! Position management and daily risk limits.
use crate::{Side, params::Params};
use chrono::NaiveDate;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ExitReason {
    /// Initial stop.
    Stop,
    /// Stop after it was moved to breakeven.
    Breakeven,
    /// Stop after it trailed beyond breakeven.
    Trail,
    Target,
    /// Not enough progress within `time_stop_bars`.
    Time,
    SquareOff,
}
impl ExitReason {
    pub fn label(self) -> &'static str {
        match self {
            ExitReason::Stop => "SL",
            ExitReason::Breakeven => "BE",
            ExitReason::Trail => "TRAIL",
            ExitReason::Target => "TARGET",
            ExitReason::Time => "TIME",
            ExitReason::SquareOff => "SQUARE_OFF",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Initial,
    Breakeven,
    Trailing,
}

/// One open position.
#[derive(Clone, Debug)]
pub struct Trade {
    pub side: Side,
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
    /// Initial risk, points.
    pub risk: f64,
    pub bars_held: u32,
    /// Best excursion so far, in R.
    pub max_r: f64,
    stage: Stage,
}

impl Trade {
    /// Opens at the actual `entry` fill with the signal's `stop` level. `None` when
    /// the fill is already through the stop or slippage made the risk too wide.
    pub fn open(side: Side, entry: f64, stop: f64, p: &Params) -> Option<Self> {
        let risk = (entry - stop) * side.dir();
        if risk <= 0.0 || risk > p.stop_cap_points * 1.25 {
            return None;
        }
        Some(Self {
            side,
            entry,
            stop,
            target: entry + side.dir() * p.target_r * risk,
            risk,
            bars_held: 0,
            max_r: 0.0,
            stage: Stage::Initial,
        })
    }
    fn stop_reason(&self) -> ExitReason {
        match self.stage {
            Stage::Initial => ExitReason::Stop,
            Stage::Breakeven => ExitReason::Breakeven,
            Stage::Trailing => ExitReason::Trail,
        }
    }
    /// Live: a price tick (bid for long, ask for short). The exit is a market order.
    pub fn on_price(&self, px: f64) -> Option<ExitReason> {
        let d = self.side.dir();
        if (px - self.stop) * d <= 0.0 {
            Some(self.stop_reason())
        } else if (px - self.target) * d >= 0.0 {
            Some(ExitReason::Target)
        } else {
            None
        }
    }
    /// Backtest: one bar's OHLC. A gap through a level fills at the open; when the
    /// bar touches both stop and target the stop is assumed first (pessimistic).
    pub fn on_bar(&self, open: f64, high: f64, low: f64) -> Option<(ExitReason, f64)> {
        let d = self.side.dir();
        let (adverse, favorable) = match self.side {
            Side::Long => (low, high),
            Side::Short => (high, low),
        };
        if (open - self.stop) * d <= 0.0 {
            return Some((self.stop_reason(), open));
        }
        if (open - self.target) * d >= 0.0 {
            return Some((ExitReason::Target, open));
        }
        if (adverse - self.stop) * d <= 0.0 {
            return Some((self.stop_reason(), self.stop));
        }
        if (favorable - self.target) * d >= 0.0 {
            return Some((ExitReason::Target, self.target));
        }
        None
    }
    /// At each bar close while open: breakeven, trail and the time stop. `swing` is
    /// the engine's lowest low (long) / highest high (short) of the last
    /// `trail_swing_bars` bars.
    pub fn on_bar_close(&mut self, high: f64, low: f64, close: f64, swing: Option<f64>, p: &Params) -> Option<ExitReason> {
        let d = self.side.dir();
        self.bars_held += 1;
        let best = match self.side {
            Side::Long => high,
            Side::Short => low,
        };
        self.max_r = self.max_r.max((best - self.entry) * d / self.risk);
        let tighten = |stop: f64, candidate: f64| if (candidate - stop) * d > 0.0 { candidate } else { stop };
        if self.stage == Stage::Initial && p.breakeven_r > 0.0 && self.max_r >= p.breakeven_r {
            self.stop = tighten(self.stop, self.entry + d * p.breakeven_offset_points);
            self.stage = Stage::Breakeven;
        }
        if p.trail_after_r > 0.0
            && self.max_r >= p.trail_after_r
            && let Some(s) = swing
            && (s - close) * d < 0.0
        {
            let moved = tighten(self.stop, s);
            if moved != self.stop {
                self.stop = moved;
                if (self.stop - self.entry) * d > p.breakeven_offset_points {
                    self.stage = Stage::Trailing;
                } else {
                    self.stage = self.stage.max(Stage::Breakeven);
                }
            }
        }
        if p.time_stop_bars > 0 && self.bars_held >= p.time_stop_bars && self.max_r < p.time_stop_min_r {
            return Some(ExitReason::Time);
        }
        None
    }
}

/// Daily limits: trade count, losing streak, net loss. Reset per IST date.
#[derive(Clone, Debug, Default)]
pub struct DayRisk {
    date: Option<NaiveDate>,
    pub trades: u32,
    pub consecutive_losses: u32,
    pub net_points: f64,
}
impl DayRisk {
    fn roll(&mut self, date: NaiveDate) {
        if self.date != Some(date) {
            *self = Self { date: Some(date), ..Self::default() };
        }
    }
    /// `Ok` when a new entry is allowed on `date`, else the reason.
    pub fn allowed(&mut self, date: NaiveDate, p: &Params) -> Result<(), &'static str> {
        self.roll(date);
        if self.trades >= p.max_trades_per_day {
            return Err("daily trade limit");
        }
        if p.max_consecutive_losses > 0 && self.consecutive_losses >= p.max_consecutive_losses {
            return Err("losing streak limit");
        }
        if p.daily_loss_limit_points > 0.0 && self.net_points <= -p.daily_loss_limit_points {
            return Err("daily loss limit");
        }
        Ok(())
    }
    pub fn on_entry(&mut self, date: NaiveDate) {
        self.roll(date);
        self.trades += 1;
    }
    /// Net points of a closed trade, after costs.
    pub fn on_exit(&mut self, date: NaiveDate, net_points: f64) {
        self.roll(date);
        self.net_points += net_points;
        if net_points < 0.0 {
            self.consecutive_losses += 1;
        } else {
            self.consecutive_losses = 0;
        }
    }
}

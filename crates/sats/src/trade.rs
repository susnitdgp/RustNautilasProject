//! Pine section 7: the single model position. The old trade is settled first
//! with the bar's full OHLC (SL wins ambiguous bars), then a new one may open.
//! Accounting is in initial-risk R, exactly like the script's trader card:
//! ⅓ at TP1, ⅓ at TP2, the remainder at TP3; SL, flip and timeout close the rest.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Side {
    Long,
    Short,
}

impl Side {
    pub fn sign(self) -> f64 {
        match self {
            Self::Long => 1.0,
            Self::Short => -1.0,
        }
    }
    pub fn trend(self) -> i8 {
        match self {
            Self::Long => 1,
            Self::Short => -1,
        }
    }
}

/// The script's event names (`buy`, `sell`, `tp1_hit` … `timeout_exit`).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Buy,
    Sell,
    Tp1Hit,
    Tp2Hit,
    Tp3Hit,
    SlHit,
    FlipExit,
    TimeoutExit,
}

impl EventKind {
    pub fn is_entry(self) -> bool {
        matches!(self, Self::Buy | Self::Sell)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
            Self::Tp1Hit => "TP1",
            Self::Tp2Hit => "TP2",
            Self::Tp3Hit => "TP3",
            Self::SlHit => "SL",
            Self::FlipExit => "FLIP",
            Self::TimeoutExit => "TIMEOUT",
        }
    }
}

/// Trade fields carried on every event (entry plan and running accounting).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct TradeSnapshot {
    pub side: Side,
    pub entry_bar: i64,
    pub entry_time_ns: i64,
    pub signal_price: f64,
    pub entry: f64,
    pub sl: f64,
    pub risk: f64,
    pub tp1: f64,
    pub tp2: f64,
    pub tp3: f64,
    pub r1: f64,
    pub r2: f64,
    pub r3: f64,
    pub hit1: bool,
    pub hit2: bool,
    pub hit3: bool,
    pub taken_r: f64,
    pub cost_r: f64,
    pub remaining: f64,
    pub entry_tqi: f64,
    pub entry_score: f64,
    pub reason: String,
    pub limitations: String,
    pub epoch: u64,
}

impl TradeSnapshot {
    pub fn net_r(&self) -> f64 {
        self.taken_r - self.cost_r
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Event {
    pub kind: EventKind,
    pub bar_index: i64,
    /// Close time of the bar the event was confirmed on (UTC ns).
    pub time_ns: i64,
    /// Level that triggered (SL / TP price, or the close for flip/timeout).
    pub level: f64,
    /// Model fill price (entries include slippage; TP fills at the limit).
    pub fill: f64,
    /// Fraction of the original position this event fills.
    pub fraction: f64,
    /// True when this event closes the model trade.
    pub closes_trade: bool,
    pub trade: TradeSnapshot,
}

/// Market data a settlement or entry needs from the current bar.
pub struct BarCtx {
    pub bar_index: i64,
    pub time_ns: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub trend: i8,
}

/// Rounding to the symbol's tick, as the script does.
#[derive(Clone, Copy, Debug)]
pub struct Ticks(pub f64);

impl Ticks {
    /// `math.round_to_mintick`: nearest tick, ties up.
    pub fn round(self, v: f64) -> f64 {
        (v / self.0 + 0.5).floor() * self.0
    }
    pub fn down(self, v: f64) -> f64 {
        (v / self.0 + 1e-9).floor() * self.0
    }
    pub fn up(self, v: f64) -> f64 {
        (v / self.0 - 1e-9).ceil() * self.0
    }
}

pub struct ExitRules {
    pub timeout_bars: i64,
    pub slip: f64,
    pub fee_pct: f64,
}

/// Settles an open trade on a confirmed bar. Returns the events and, when the
/// trade closed, its final net R.
pub fn settle(t: &mut TradeSnapshot, bar: &BarCtx, rules: &ExitRules) -> (Vec<Event>, Option<f64>) {
    let mut events = Vec::new();
    if bar.bar_index <= t.entry_bar {
        return (events, None);
    }
    let side = t.side;
    let s = side.sign();
    let reached = |level: f64| if side == Side::Long { bar.high >= level } else { bar.low <= level };
    let stop_hit = if t.side == Side::Long { bar.low <= t.sl } else { bar.high >= t.sl };
    let opposite = t.side.trend() != bar.trend;
    let timed_out = bar.bar_index - t.entry_bar >= rules.timeout_bars;
    let mut closed = false;
    let mut push = |t: &TradeSnapshot, kind, level, fill, fraction, closes| {
        events.push(Event {
            kind,
            bar_index: bar.bar_index,
            time_ns: bar.time_ns,
            level,
            fill,
            fraction,
            closes_trade: closes,
            trade: t.clone(),
        });
    };
    if stop_hit {
        let base = if t.side == Side::Long { bar.open.min(t.sl) } else { bar.open.max(t.sl) };
        let fill = base - s * rules.slip;
        let exiting = t.remaining;
        take_fill(t, fill, exiting, rules.fee_pct);
        push(t, EventKind::SlHit, t.sl, fill, exiting, true);
        closed = true;
    } else {
        if !t.hit1 && reached(t.tp1) {
            t.hit1 = true;
            take_fill(t, t.tp1, 1.0 / 3.0, rules.fee_pct);
            push(t, EventKind::Tp1Hit, t.tp1, t.tp1, 1.0 / 3.0, false);
        }
        if !t.hit2 && reached(t.tp2) {
            t.hit2 = true;
            take_fill(t, t.tp2, 1.0 / 3.0, rules.fee_pct);
            push(t, EventKind::Tp2Hit, t.tp2, t.tp2, 1.0 / 3.0, false);
        }
        if !t.hit3 && reached(t.tp3) {
            t.hit3 = true;
            let exiting = t.remaining;
            take_fill(t, t.tp3, exiting, rules.fee_pct);
            push(t, EventKind::Tp3Hit, t.tp3, t.tp3, exiting, true);
            closed = true;
        } else if opposite || timed_out {
            let fill = bar.close - s * rules.slip;
            let exiting = t.remaining;
            take_fill(t, fill, exiting, rules.fee_pct);
            let kind = if opposite { EventKind::FlipExit } else { EventKind::TimeoutExit };
            push(t, kind, bar.close, fill, exiting, true);
            closed = true;
        }
    }
    (events, closed.then(|| t.net_r()))
}

fn take_fill(t: &mut TradeSnapshot, fill: f64, fraction: f64, fee_pct: f64) {
    t.taken_r += fraction * t.side.sign() * (fill - t.entry) / t.risk;
    t.cost_r += fraction * fill.abs() * fee_pct / 100.0 / t.risk;
    t.remaining = (t.remaining - fraction).max(0.0);
}

/// Everything the entry block needs.
pub struct EntryInputs {
    pub side: Side,
    pub close: f64,
    pub low: f64,
    pub high: f64,
    pub atr_value: f64,
    pub sl_mult: f64,
    pub max_sl_dist: f64,
    pub last_pivot_low: Option<f64>,
    pub last_pivot_high: Option<f64>,
    pub valid_low_pivot: bool,
    pub valid_high_pivot: bool,
    pub live_r: [f64; 3],
    pub dyn_floors: Option<[f64; 3]>,
    pub dyn_ceiling: f64,
    pub min_risk_ticks: u32,
    pub slip: f64,
    pub fee_pct: f64,
    pub ticks: Ticks,
}

/// Builds and validates the entry plan. `Err` carries the script's rejection text.
pub fn plan(i: &EntryInputs) -> Result<PlanLevels, String> {
    let d = i.side.sign();
    let tick = i.ticks.0;
    let entry = i.ticks.round(i.close + d * i.slip);
    let usable = if i.side == Side::Long { i.valid_low_pivot } else { i.valid_high_pivot };
    let pivot_base = match i.side {
        Side::Long if usable => i.last_pivot_low.unwrap_or(i.low),
        Side::Long => i.low,
        Side::Short if usable => i.last_pivot_high.unwrap_or(i.high),
        Side::Short => i.high,
    };
    let buffer = i.sl_mult * i.atr_value;
    let cap = i.max_sl_dist.max(i.sl_mult) * i.atr_value;
    let raw_stop = if i.side == Side::Long {
        (pivot_base - buffer).min(entry - buffer).max(entry - cap)
    } else {
        (pivot_base + buffer).max(entry + buffer).min(entry + cap)
    };
    let stop = if i.side == Side::Long { i.ticks.down(raw_stop) } else { i.ticks.up(raw_stop) };
    let risk = d * (entry - stop);
    let floor = |k: usize| i.dyn_floors.map_or(0.0, |f| i.ticks.up(risk * f[k]));
    let d1 = tick.max(i.ticks.down(risk * i.live_r[0]).max(floor(0)));
    let d2 = d1.max(i.ticks.down(risk * i.live_r[1]).max(floor(1)));
    let d3 = d2.max(i.ticks.down(risk * i.live_r[2]).max(floor(2)));
    let tp1 = i.ticks.round(entry + d * d1);
    let tp2 = i.ticks.round(entry + d * d2);
    let tp3 = i.ticks.round(entry + d * d3);
    let tol = tick * 1e-6;
    let ceiling_ok = i.dyn_floors.is_none() || d3 <= risk * i.dyn_ceiling + tol;
    let stop_live = d * (i.close - stop) > tol;
    let valid = stop_live
        && risk >= f64::from(i.min_risk_ticks) * tick - tol
        && d * (tp1 - entry) >= tick - tol
        && d * (tp2 - tp1) >= -tol
        && d * (tp3 - tp2) >= -tol
        && ceiling_ok;
    if !valid {
        let side = if i.side == Side::Long { "BUY" } else { "SELL" };
        return Err(if stop_live {
            format!("Rejected {side}: invalid/sub-tick risk or targets")
        } else {
            format!("Rejected {side}: SL already breached at signal close after slippage")
        });
    }
    Ok(PlanLevels {
        entry,
        sl: stop,
        risk,
        tp: [tp1, tp2, tp3],
        r: [d * (tp1 - entry) / risk, d * (tp2 - entry) / risk, d * (tp3 - entry) / risk],
        pivot_usable: usable,
        capped: risk >= cap,
        cost_r: entry.abs() * i.fee_pct / 100.0 / risk,
    })
}

pub struct PlanLevels {
    pub entry: f64,
    pub sl: f64,
    pub risk: f64,
    pub tp: [f64; 3],
    pub r: [f64; 3],
    pub pivot_usable: bool,
    pub capped: bool,
    pub cost_r: f64,
}

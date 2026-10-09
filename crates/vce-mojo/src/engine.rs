//! VCE-Mojo v1.6 state machine: Pine SECTIONS 4–9, 13 and 15.
//!
//! Every step below is commented with the Pine section it mirrors. Drawing,
//! dashboard and alert formatting (SECTIONS 10–12, 14, 16, 17) are not logic and
//! are left to the caller.
use crate::indicators::{Atr, RollingMedian};
use crate::params::{ExitTarget, Params, Resolved, TouchMode};
use chrono::{DateTime, FixedOffset, NaiveDate, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// One completed candle. Times are UTC epoch nanoseconds of the bar's open and close.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub struct BarInput {
    pub open_time_ns: i64,
    pub close_time_ns: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Side {
    Long,
    Short,
}

/// The four AlgoMojo actions the Pine script alerts on.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Action {
    #[serde(rename = "BUY")]
    Buy,
    #[serde(rename = "SELL")]
    Sell,
    #[serde(rename = "SHORT")]
    Short,
    #[serde(rename = "COVER")]
    Cover,
}

impl Action {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
            Self::Short => "SHORT",
            Self::Cover => "COVER",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ExitReason {
    StopLoss,
    Target(ExitTarget),
    EndOfDay,
}

impl ExitReason {
    pub const fn label(self) -> &'static str {
        match self {
            Self::StopLoss => "SL",
            Self::Target(t) => t.label(),
            Self::EndOfDay => "EOD",
        }
    }
}

/// An open trade: SECTION 8 trade state plus SECTION 9 levels.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub struct Levels {
    pub side: Side,
    pub entry: f64,
    pub sl: f64,
    pub tp1: f64,
    pub tp2: f64,
    pub tp3: f64,
    /// Final stop distance (1R) after the min/max ATR clamp.
    pub risk: f64,
    pub exit_target: ExitTarget,
    pub entry_bar: u64,
    pub entry_time_ns: i64,
    pub coil_bars: usize,
    pub tp1_hit: bool,
    pub tp2_hit: bool,
}

impl Levels {
    pub fn target_price(&self) -> f64 {
        match self.exit_target {
            ExitTarget::Tp1 => self.tp1,
            ExitTarget::Tp2 => self.tp2,
            ExitTarget::Tp3 => self.tp3,
        }
    }
    fn reached(&self, level: f64, high: f64, low: f64) -> bool {
        match self.side {
            Side::Long => high >= level,
            Side::Short => low <= level,
        }
    }
    fn stopped(&self, high: f64, low: f64) -> bool {
        match self.side {
            Side::Long => low <= self.sl,
            Side::Short => high >= self.sl,
        }
    }
    /// SECTION 15 milestone dots (targets below the selected exit target).
    fn mark_milestones(&mut self, high: f64, low: f64) {
        let idx = self.exit_target.index();
        if idx > 1 && !self.tp1_hit && self.reached(self.tp1, high, low) {
            self.tp1_hit = true;
        }
        if idx > 2 && !self.tp2_hit && self.reached(self.tp2, high, low) {
            self.tp2_hit = true;
        }
    }
    /// SL beats the target when both are touched within the same bar.
    fn exit_reason(&self, high: f64, low: f64) -> Option<ExitReason> {
        if self.stopped(high, low) {
            Some(ExitReason::StopLoss)
        } else if self.reached(self.target_price(), high, low) {
            Some(ExitReason::Target(self.exit_target))
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum Event {
    /// BUY or SHORT, confirmed at bar close. `price` is the signal bar's close.
    Entry {
        action: Action,
        bar_index: u64,
        time_ns: i64,
        price: f64,
        levels: Levels,
    },
    /// SELL or COVER. For bar-close exits `price` is the Pine reference level
    /// (SL / target) or the close (EOD); for intrabar exits it is the touching price.
    Exit {
        action: Action,
        bar_index: u64,
        time_ns: i64,
        reason: ExitReason,
        price: f64,
        intrabar: bool,
        levels: Levels,
    },
}

impl Event {
    pub fn action(&self) -> Action {
        match self {
            Self::Entry { action, .. } | Self::Exit { action, .. } => *action,
        }
    }
}

/// Snapshot matching the Pine dashboard's STATE / COIL rows.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Status {
    pub state: String,
    pub bars: u64,
    pub session_bars: u32,
    pub coil_bars: usize,
    pub watch_active: bool,
    pub watch_is_short: bool,
    pub watch_high: Option<f64>,
    pub watch_low: Option<f64>,
    pub trade: Option<Levels>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct Hist {
    high: f64,
    low: f64,
    bg_atr: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Engine {
    params: Params,
    res: Resolved,
    // SECTION 4
    atr: Atr,
    bg_atr: Atr,
    local_atr: Atr,
    median_range: RollingMedian,
    /// Newest first: index 0 is the current bar, index k is Pine's `[k]`.
    hist: VecDeque<Hist>,
    hist_cap: usize,
    bars_seen: u64,
    prev_day: Option<NaiveDate>,
    sess_bars: u32,
    last_eod_bar: bool,
    // SECTION 5
    comp_len: usize,
    comp_high: Option<f64>,
    comp_low: Option<f64>,
    comp_violations: u32,
    // SECTION 6
    watch_active: bool,
    watch_is_short: bool,
    watch_high: Option<f64>,
    watch_low: Option<f64>,
    watch_len: usize,
    watch_end_bar: Option<u64>,
    // SECTIONS 7–8
    last_outcome_bar: i64,
    trade: Option<Levels>,
}

impl Engine {
    pub fn new(params: Params) -> Result<Self, String> {
        params.validate()?;
        let res = params.resolve();
        Ok(Self {
            atr: Atr::new(params.atr_period),
            bg_atr: Atr::new(params.bg_atr_period),
            local_atr: Atr::new(res.comp_min_bars.max(3)),
            median_range: RollingMedian::new(50),
            hist_cap: params.session_lookback.max(6) + 1,
            hist: VecDeque::new(),
            bars_seen: 0,
            prev_day: None,
            sess_bars: 0,
            last_eod_bar: false,
            comp_len: 0,
            comp_high: None,
            comp_low: None,
            comp_violations: 0,
            watch_active: false,
            watch_is_short: false,
            watch_high: None,
            watch_low: None,
            watch_len: 0,
            watch_end_bar: None,
            last_outcome_bar: -100,
            trade: None,
            params,
            res,
        })
    }

    pub fn params(&self) -> &Params {
        &self.params
    }
    pub fn position(&self) -> Option<&Levels> {
        self.trade.as_ref()
    }
    /// Number of completed bars processed (Pine `bar_index` of the next bar).
    pub fn bars_seen(&self) -> u64 {
        self.bars_seen
    }

    fn offset(&self) -> FixedOffset {
        FixedOffset::east_opt(self.params.utc_offset_minutes * 60).expect("validated offset")
    }

    /// Process one completed bar with Pine's bar-close semantics.
    pub fn on_bar(&mut self, bar: &BarInput) -> Vec<Event> {
        let idx = self.bars_seen;
        self.bars_seen += 1;
        let (o, h, l, c) = (bar.open, bar.high, bar.low, bar.close);
        let mut events = Vec::new();
        let zone = self.offset();

        // ── SECTION 4 · new trading day, warm-up, ATRs, session range, EOD clock ──
        let day = DateTime::from_timestamp_nanos(bar.open_time_ns)
            .with_timezone(&zone)
            .date_naive();
        let new_session = self.prev_day.is_some_and(|d| d != day);
        self.prev_day = Some(day);
        self.sess_bars = if new_session { 1 } else { self.sess_bars + 1 };
        let sess_ready = self.sess_bars > self.params.session_warmup_bars;

        let atr = self.atr.update(h, l, c);
        let bg_atr = self.bg_atr.update(h, l, c);
        let local_atr = self.local_atr.update(h, l, c);
        let median_rng = self.median_range.update(h - l);

        self.hist.push_front(Hist {
            high: h,
            low: l,
            bg_atr,
        });
        self.hist.truncate(self.hist_cap);

        let range_len = if self.sess_bars <= 1 {
            1
        } else {
            ((self.sess_bars - 1) as usize)
                .min(self.params.session_lookback)
                .max(1)
        };
        let window = self.hist.iter().take(range_len);
        let session_hi = window
            .clone()
            .map(|x| x.high)
            .fold(f64::NEG_INFINITY, f64::max);
        let session_lo = window.map(|x| x.low).fold(f64::INFINITY, f64::min);
        let session_rng = session_hi - session_lo;

        let close_local = DateTime::from_timestamp_nanos(bar.close_time_ns).with_timezone(&zone);
        let close_min = close_local.hour() * 60 + close_local.minute();
        let eod_bar = self.params.eod_square_off && close_min >= self.params.cutoff_minute();
        self.last_eod_bar = eod_bar;

        // ── SECTION 5 · coil detection ──
        if new_session {
            self.reset_coil();
        }
        let below =
            |factor: f64| matches!((local_atr, bg_atr), (Some(la), Some(b)) if la < b * factor);
        let ratio_contracted = below(self.res.atr_contraction);
        let drift_contracted = median_rng.is_some_and(|m| (h - l) < m * 0.65);
        let mut is_contracted = ratio_contracted || drift_contracted;
        if self.params.catalyst_mode {
            let recent_expansion = (1..=5).any(|k| {
                self.hist
                    .get(k)
                    .is_some_and(|x| x.bg_atr.is_some_and(|b| (x.high - x.low) > b * 1.5))
            });
            is_contracted = is_contracted || (recent_expansion && below(1.10));
        }

        if is_contracted {
            if self.comp_len == 0 {
                self.start_coil(h, l);
            } else {
                let in_range = match (bg_atr, self.comp_high, self.comp_low) {
                    (Some(b), Some(ch), Some(cl)) => {
                        let tol = b * self.res.tol_mult;
                        h <= ch + tol && l >= cl - tol
                    }
                    _ => false,
                };
                if in_range {
                    self.extend_coil(h, l);
                } else {
                    self.comp_violations += 1;
                    if self.comp_violations <= self.res.max_violations {
                        self.extend_coil(h, l);
                    } else {
                        self.start_coil(h, l);
                    }
                }
            }
        } else {
            if self.comp_len > 0 {
                self.comp_len -= 1;
            }
            if self.comp_len == 0 {
                self.reset_coil();
            }
        }
        if self.comp_len > self.res.comp_max_bars {
            self.reset_coil();
        }

        let comp_qualified = self.comp_len >= self.res.comp_min_bars;
        let zone_top = session_hi - session_rng * self.res.extreme_zone;
        let zone_bot = session_lo + session_rng * self.res.extreme_zone;
        let (mut comp_at_high, mut comp_at_low) = (false, false);
        if comp_qualified && let (Some(ch), Some(cl)) = (self.comp_high, self.comp_low) {
            let mid = (ch + cl) / 2.0;
            match self.res.touch_mode {
                TouchMode::Full => {
                    comp_at_high = cl >= zone_top;
                    comp_at_low = ch <= zone_bot;
                }
                TouchMode::Edge => {
                    comp_at_high = ch >= zone_top;
                    comp_at_low = cl <= zone_bot;
                }
                TouchMode::Mid => {
                    comp_at_high = ch >= zone_top || mid >= zone_top || c >= zone_top;
                    comp_at_low = cl <= zone_bot || mid <= zone_bot || c <= zone_bot;
                }
            }
        }

        // ── SECTION 6 · watched coil ──
        if new_session {
            self.watch_active = false;
            self.watch_is_short = false;
            self.clear_watch();
        }
        if comp_at_high && !self.watch_active && sess_ready {
            self.start_watch(true);
        } else if comp_at_low && !self.watch_active && sess_ready {
            self.start_watch(false);
        }
        if self.watch_active
            && comp_qualified
            && ((self.watch_is_short && comp_at_high) || (!self.watch_is_short && comp_at_low))
            && let (Some(wh), Some(wl), Some(ch), Some(cl)) = (
                self.watch_high,
                self.watch_low,
                self.comp_high,
                self.comp_low,
            )
        {
            self.watch_high = Some(wh.max(ch));
            self.watch_low = Some(wl.min(cl));
            self.watch_len = self.comp_len;
        }
        if self.watch_active && !comp_qualified && self.watch_end_bar.is_none() {
            self.watch_end_bar = Some(idx);
        }
        if self.watch_active
            && self
                .watch_end_bar
                .is_some_and(|e| idx - e > self.res.watch_expiry)
        {
            self.watch_active = false;
            self.clear_watch();
        }

        // ── SECTION 7 · triggers (bar-close confirmed) ──
        let gate_ok = idx as i64 - self.last_outcome_bar > i64::from(self.params.post_outcome_gap);
        let (short_break, long_break) = match self.res.touch_mode {
            TouchMode::Full => (
                self.watch_low.is_some_and(|wl| c < wl),
                self.watch_high.is_some_and(|wh| c > wh),
            ),
            TouchMode::Edge => (
                self.watch_low
                    .is_some_and(|wl| c < wl || (l < wl && c < o && (o - c) > (h - l) * 0.40)),
                self.watch_high
                    .is_some_and(|wh| c > wh || (h > wh && c > o && (c - o) > (h - l) * 0.40)),
            ),
            TouchMode::Mid => (
                self.watch_low
                    .is_some_and(|wl| c < wl || bg_atr.is_some_and(|b| l < wl - b * 0.05)),
                self.watch_high
                    .is_some_and(|wh| c > wh || bg_atr.is_some_and(|b| h > wh + b * 0.05)),
            ),
        };
        let short_trigger = self.watch_active && self.watch_is_short && short_break && gate_ok;
        let long_trigger = self.watch_active && !self.watch_is_short && long_break && gate_ok;

        // ── SECTION 13 · entries (BUY / SHORT); none on/after the EOD cut-off bar ──
        // Pine would compute na levels if ATR(14) were still na; such a trade could
        // never exit, so the port requires a valid ATR before entering.
        let trade_active = self.trade.is_some();
        let fire_long = long_trigger && !trade_active && !eod_bar && atr.is_some();
        let fire_short = short_trigger && !trade_active && !eod_bar && atr.is_some();
        if (fire_long || fire_short)
            && let (Some(atr), Some(wh), Some(wl)) = (atr, self.watch_high, self.watch_low)
        {
            let levels = self.calc_levels(fire_long, c, wh, wl, atr, idx, bar.close_time_ns);
            self.trade = Some(levels);
            self.watch_active = false;
            events.push(Event::Entry {
                action: if fire_long {
                    Action::Buy
                } else {
                    Action::Short
                },
                bar_index: idx,
                time_ns: bar.close_time_ns,
                price: c,
                levels,
            });
        }

        // ── SECTION 15 · exits (SELL / COVER) ──
        let mut exit: Option<(ExitReason, f64)> = None;
        if let Some(t) = self.trade.as_mut()
            && idx > t.entry_bar
        {
            t.mark_milestones(h, l);
            exit = t.exit_reason(h, l).map(|r| {
                let px = if r == ExitReason::StopLoss {
                    t.sl
                } else {
                    t.target_price()
                };
                (r, px)
            });
        }
        if self.trade.is_some() && exit.is_none() && eod_bar {
            exit = Some((ExitReason::EndOfDay, c));
        }
        if let Some((reason, price)) = exit
            && let Some(levels) = self.trade.take()
        {
            self.last_outcome_bar = idx as i64;
            events.push(Event::Exit {
                action: exit_action(levels.side),
                bar_index: idx,
                time_ns: bar.close_time_ns,
                reason,
                price,
                intrabar: false,
                levels,
            });
        }
        events
    }

    /// Live intrabar check: Pine fires SL / target exit alerts on the first touch,
    /// before the bar closes. Call with every trade price of the forming bar.
    /// The bar's later `on_bar` then sees no open trade, and the post-outcome
    /// gap blocks a new entry on that bar, as on TradingView.
    pub fn on_price(&mut self, price: f64, time_ns: i64) -> Option<Event> {
        let forming = self.bars_seen;
        let t = self.trade.as_mut()?;
        if forming <= t.entry_bar || !price.is_finite() {
            return None;
        }
        t.mark_milestones(price, price);
        let reason = t.exit_reason(price, price)?;
        let levels = self.trade.take()?;
        self.last_outcome_bar = forming as i64;
        Some(Event::Exit {
            action: exit_action(levels.side),
            bar_index: forming,
            time_ns,
            reason,
            price,
            intrabar: true,
            levels,
        })
    }

    /// Pine dashboard STATE row logic.
    pub fn status(&self) -> Status {
        let state = if let Some(t) = &self.trade {
            if t.side == Side::Long {
                "ACTIVE LONG".into()
            } else {
                "ACTIVE SHORT".into()
            }
        } else if self.last_eod_bar {
            "EOD · NO ENTRY".into()
        } else if self.sess_bars <= self.params.session_warmup_bars {
            format!(
                "WARM-UP {}/{}",
                self.sess_bars, self.params.session_warmup_bars
            )
        } else if self.watch_active {
            if self.watch_is_short {
                "WATCH SHORT".into()
            } else {
                "WATCH LONG".into()
            }
        } else if self.comp_len > 0 {
            format!("FORMING {}b", self.comp_len)
        } else {
            "IDLE".into()
        };
        Status {
            state,
            bars: self.bars_seen,
            session_bars: self.sess_bars,
            coil_bars: self.comp_len,
            watch_active: self.watch_active,
            watch_is_short: self.watch_is_short,
            watch_high: self.watch_high,
            watch_low: self.watch_low,
            trade: self.trade,
        }
    }

    // ── SECTION 9 · stop and targets ──
    #[allow(clippy::too_many_arguments)]
    fn calc_levels(
        &self,
        long: bool,
        entry: f64,
        comp_hi: f64,
        comp_lo: f64,
        atr: f64,
        bar: u64,
        time_ns: i64,
    ) -> Levels {
        let p = &self.params;
        let buffer = atr * p.sl_buffer_atr;
        let min_dist = atr * p.sl_min_dist_atr;
        let max_dist = atr * p.sl_max_dist_atr;
        let raw_sl = if long {
            comp_lo - buffer
        } else {
            comp_hi + buffer
        };
        let raw_dist = (entry - raw_sl).abs();
        let dist = min_dist.max(raw_dist.min(max_dist));
        let dir = if long { 1.0 } else { -1.0 };
        Levels {
            side: if long { Side::Long } else { Side::Short },
            entry,
            sl: entry - dir * dist,
            tp1: entry + dir * dist * p.tp1_r,
            tp2: entry + dir * dist * p.tp2_r,
            tp3: entry + dir * dist * p.tp3_r,
            risk: dist,
            exit_target: p.exit_target,
            entry_bar: bar,
            entry_time_ns: time_ns,
            coil_bars: self.watch_len,
            tp1_hit: false,
            tp2_hit: false,
        }
    }

    fn start_coil(&mut self, h: f64, l: f64) {
        self.comp_len = 1;
        self.comp_high = Some(h);
        self.comp_low = Some(l);
        self.comp_violations = 0;
    }
    fn extend_coil(&mut self, h: f64, l: f64) {
        self.comp_len += 1;
        self.comp_high = self.comp_high.map(|x| x.max(h));
        self.comp_low = self.comp_low.map(|x| x.min(l));
    }
    fn reset_coil(&mut self) {
        self.comp_len = 0;
        self.comp_high = None;
        self.comp_low = None;
        self.comp_violations = 0;
    }
    fn start_watch(&mut self, short: bool) {
        self.watch_active = true;
        self.watch_is_short = short;
        self.watch_high = self.comp_high;
        self.watch_low = self.comp_low;
        self.watch_len = self.comp_len;
        self.watch_end_bar = None;
    }
    fn clear_watch(&mut self) {
        self.watch_high = None;
        self.watch_low = None;
        self.watch_len = 0;
        self.watch_end_bar = None;
    }
}

const fn exit_action(side: Side) -> Action {
    match side {
        Side::Long => Action::Sell,
        Side::Short => Action::Cover,
    }
}

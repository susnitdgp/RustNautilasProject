//! Signal model: closed 1m bars in, entry signals out.
use crate::{
    Side,
    indicators::{Atr, Vwap, median},
    params::Params,
};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};

fn ist() -> FixedOffset {
    FixedOffset::east_opt(19_800).expect("IST")
}

/// One closed bar. `start` is epoch seconds (UTC) of the bar's open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub start: i64,
    pub seconds: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}
impl Bar {
    pub fn close_time(&self) -> i64 {
        self.start + self.seconds
    }
    fn ist(ts: i64) -> DateTime<FixedOffset> {
        DateTime::from_timestamp(ts, 0).expect("timestamp").with_timezone(&ist())
    }
    pub fn session_date(&self) -> NaiveDate {
        Self::ist(self.start).date_naive()
    }
    pub fn close_time_ist(&self) -> NaiveTime {
        Self::ist(self.close_time()).time()
    }
}

/// An entry decided at a bar close; the order goes in right after.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Signal {
    pub side: Side,
    /// Epoch seconds of the signal bar's close.
    pub bar_close: i64,
    pub close: f64,
    /// Protective stop level (absolute price).
    pub stop: f64,
    /// close → stop distance, points.
    pub risk: f64,
    pub atr: f64,
    pub volume_ratio: f64,
    pub vwap: f64,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Engine {
    p: Params,
    atr: Atr,
    vwap: Vwap,
    recent: VecDeque<Bar>,
    keep: usize,
    session: Option<NaiveDate>,
    session_start: i64,
    session_bars: usize,
    day_high: f64,
    day_low: f64,
    prev_day: Option<(f64, f64)>,
    opening_range: Option<(f64, f64)>,
    /// Why bars were not signals (after warm-up): the tuning funnel.
    rejects: BTreeMap<&'static str, u64>,
}

impl Engine {
    pub fn new(p: Params) -> Self {
        let keep = p.base_bars.max(p.volume_lookback).max(p.trail_swing_bars) + 1;
        Self {
            atr: Atr::new(p.atr_length),
            vwap: Vwap::default(),
            recent: VecDeque::with_capacity(keep + 1),
            keep,
            session: None,
            session_start: 0,
            session_bars: 0,
            day_high: f64::MIN,
            day_low: f64::MAX,
            prev_day: None,
            opening_range: None,
            rejects: BTreeMap::new(),
            p,
        }
    }
    pub fn params(&self) -> &Params {
        &self.p
    }
    pub fn vwap(&self) -> f64 {
        self.vwap.value()
    }
    pub fn atr(&self) -> Option<f64> {
        self.atr.value()
    }
    /// Count of bars rejected per rule since start (first failing rule only).
    pub fn rejects(&self) -> &BTreeMap<&'static str, u64> {
        &self.rejects
    }
    fn reject(&mut self, why: &'static str) -> Option<Signal> {
        *self.rejects.entry(why).or_default() += 1;
        None
    }
    /// Lowest low (long) / highest high (short) of the last `n` bars, including the
    /// latest — the swing the trailing stop follows.
    pub fn swing(&self, side: Side, n: usize) -> Option<f64> {
        let bars: Vec<_> = self.recent.iter().rev().take(n).collect();
        if bars.is_empty() {
            return None;
        }
        Some(match side {
            Side::Long => bars.iter().map(|b| b.low).fold(f64::MAX, f64::min),
            Side::Short => bars.iter().map(|b| b.high).fold(f64::MIN, f64::max),
        })
    }

    /// Feed one closed bar; returns an entry signal when every rule agrees.
    pub fn on_bar(&mut self, bar: Bar) -> Option<Signal> {
        let date = bar.session_date();
        if self.session != Some(date) {
            if self.session.is_some() && self.day_high >= self.day_low {
                self.prev_day = Some((self.day_high, self.day_low));
            }
            self.session = Some(date);
            self.session_start = bar.start;
            self.session_bars = 0;
            self.day_high = f64::MIN;
            self.day_low = f64::MAX;
            self.opening_range = None;
            self.vwap.reset();
        }
        // Everything the rules compare against comes from BEFORE this bar.
        let atr_before = self.atr.value();
        let base: Vec<Bar> = if self.session_bars >= self.p.base_bars {
            self.recent.iter().rev().take(self.p.base_bars).copied().collect()
        } else {
            Vec::new()
        };
        let volumes: Vec<f64> = self.recent.iter().rev().take(self.p.volume_lookback).map(|b| b.volume).collect();
        let or_complete = bar.start >= self.session_start + self.p.opening_range_minutes * 60;
        let levels: Vec<f64> = self
            .prev_day
            .iter()
            .flat_map(|(h, l)| [*h, *l])
            .chain(self.opening_range.filter(|_| or_complete).iter().flat_map(|(h, l)| [*h, *l]))
            .collect();

        // Update state with this bar.
        self.atr.update(bar.high, bar.low, bar.close);
        let vwap = self.vwap.update(bar.high, bar.low, bar.close, bar.volume);
        self.day_high = self.day_high.max(bar.high);
        self.day_low = self.day_low.min(bar.low);
        if !or_complete {
            self.opening_range = Some(match self.opening_range {
                Some((h, l)) => (h.max(bar.high), l.min(bar.low)),
                None => (bar.high, bar.low),
            });
        }
        self.session_bars += 1;
        self.recent.push_back(bar);
        while self.recent.len() > self.keep {
            self.recent.pop_front();
        }

        let atr = atr_before?;
        if base.len() < self.p.base_bars || volumes.len() < self.p.volume_lookback || atr <= 0.0 {
            return None;
        }
        let t = bar.close_time_ist();
        if t < self.p.entry_from || t >= self.p.entry_to {
            return self.reject("1 outside entry window");
        }
        let base_high = base.iter().map(|b| b.high).fold(f64::MIN, f64::max);
        let base_low = base.iter().map(|b| b.low).fold(f64::MAX, f64::min);
        if base_high - base_low > self.p.base_max_atr * atr {
            return self.reject("2 base too wide");
        }
        let range = bar.high - bar.low;
        if range < self.p.breakout_min_atr * atr || range <= 0.0 {
            return self.reject("3 bar too small");
        }
        let med = median(&volumes)?;
        if med <= 0.0 || bar.volume < self.p.volume_mult * med {
            return self.reject("4 volume too low");
        }
        let close_pos = (bar.close - bar.low) / range;
        let side = if bar.close > base_high
            && close_pos >= self.p.close_position
            && (!self.p.require_vwap || bar.close > vwap)
        {
            Side::Long
        } else if bar.close < base_low
            && close_pos <= 1.0 - self.p.close_position
            && (!self.p.require_vwap || bar.close < vwap)
        {
            Side::Short
        } else {
            return self.reject("5 no breakout close / wrong side of VWAP");
        };
        let raw_stop = match side {
            Side::Long => base_low - self.p.stop_buffer_points,
            Side::Short => base_high + self.p.stop_buffer_points,
        };
        let mut risk = (bar.close - raw_stop) * side.dir();
        if risk > self.p.stop_cap_points {
            return self.reject("6 stop wider than cap");
        }
        risk = risk.max(self.p.stop_min_points);
        let stop = bar.close - side.dir() * risk;
        if self.p.room_mult > 0.0 {
            let target_dist = self.p.target_r * risk;
            let nearest = levels
                .iter()
                .map(|l| (l - bar.close) * side.dir())
                .filter(|d| *d > 0.0)
                .fold(f64::MAX, f64::min);
            if nearest < self.p.room_mult * target_dist {
                return self.reject("7 no room to next level");
            }
        }
        Some(Signal {
            side,
            bar_close: bar.close_time(),
            close: bar.close,
            stop,
            risk,
            atr,
            volume_ratio: bar.volume / med,
            vwap,
            reason: format!(
                "{} burst: base {:.0}-{:.0} ({:.1} ATR), bar {:.1} ATR, vol {:.1}x, close {:.0}% of range",
                side.label(),
                base_low,
                base_high,
                (base_high - base_low) / atr,
                range / atr,
                bar.volume / med,
                close_pos * 100.0
            ),
        })
    }
}

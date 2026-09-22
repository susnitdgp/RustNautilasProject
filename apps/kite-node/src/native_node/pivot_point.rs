// This source code is subject to the Mozilla Public License 2.0: https://mozilla.org/MPL/2.0/
// Original Pivot Point SuperTrend Pine Script © LonesomeTheBlue.
// Rust port: causal completed bars, explicit session reset and position-based exits.
use super::{
    pivot_session::{self, BAR_NS, Session},
    session_calendar::Calendar,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub pivot_period: usize,
    pub atr_period: usize,
    pub atr_factor: f64,
    pub session: Session,
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=50).contains(&self.pivot_period),
            "Pivot period must be 1..50"
        );
        ensure!(
            (1..=10_000).contains(&self.atr_period),
            "Pivot ATR period must be 1..10000"
        );
        ensure!(
            self.atr_factor.is_finite() && self.atr_factor >= 1.,
            "Pivot ATR factor must be finite and at least 1"
        );
        self.session.validate()
    }
}
/// Pine ta.atr = RMA(true range), seeded by the first period TR arithmetic mean.
/// This intentionally differs from the existing Nautilus first-TR-seeded ATR.
#[derive(Debug)]
struct Atr {
    period: usize,
    count: usize,
    sum: f64,
    previous_close: Option<f64>,
    value: Option<f64>,
}
impl Atr {
    fn new(period: usize) -> Self {
        Self {
            period,
            count: 0,
            sum: 0.,
            previous_close: None,
            value: None,
        }
    }
    fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let range = self.previous_close.map_or(high - low, |p| {
            (high - low).max((high - p).abs()).max((low - p).abs())
        });
        self.previous_close = Some(close);
        self.value = match self.value {
            Some(previous) => {
                Some((previous * (self.period - 1) as f64 + range) / self.period as f64)
            }
            None => {
                self.count += 1;
                self.sum += range;
                (self.count == self.period).then(|| self.sum / self.period as f64)
            }
        };
        self.value
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    pub bar_close_ns: u64,
    pub close: f64,
    pub atr: Option<f64>,
    pub center: Option<f64>,
    pub pivot_high: Option<f64>,
    pub pivot_low: Option<f64>,
    pub support: Option<f64>,
    pub resistance: Option<f64>,
    pub supertrend: Option<f64>,
    pub direction: i8,
    pub signal: i8,
    pub initialized: bool,
    pub in_session: bool,
    pub new_session: bool,
}
#[derive(Debug)]
pub struct PivotPoint {
    pub settings: Settings,
    calendar: Calendar,
    atr: Atr,
    window: VecDeque<(f64, f64)>,
    center: Option<f64>,
    up: Option<f64>,
    down: Option<f64>,
    previous_close: Option<f64>,
    trend: i8,
    session_date: Option<chrono::NaiveDate>,
    support: Option<f64>,
    resistance: Option<f64>,
    last_bar: u64,
}
impl PivotPoint {
    pub fn new(settings: Settings, calendar: Calendar) -> Result<Self> {
        settings.validate()?;
        calendar.validate()?;
        Ok(Self {
            atr: Atr::new(settings.atr_period),
            settings,
            calendar,
            window: VecDeque::new(),
            center: None,
            up: None,
            down: None,
            previous_close: None,
            trend: 0,
            session_date: None,
            support: None,
            resistance: None,
            last_bar: 0,
        })
    }
    pub fn rebuild_empty(&self) -> Result<Self> {
        Self::new(self.settings.clone(), self.calendar.clone())
    }
    pub fn in_session(&self, ns: u64) -> Result<bool> {
        self.settings.session.contains(ns, &self.calendar)
    }
    fn reset_session(&mut self) {
        self.window.clear();
        self.center = None;
        self.up = None;
        self.down = None;
        self.previous_close = None;
        self.trend = 0;
        self.support = None;
        self.resistance = None;
        // ATR is an independent continuous ta.atr series, as in the supplied Pine script.
    }
    pub fn update(
        &mut self,
        high: f64,
        low: f64,
        close: f64,
        bar_close_ns: u64,
    ) -> Result<Observation> {
        ensure!(
            bar_close_ns >= BAR_NS
                && bar_close_ns > self.last_bar
                && bar_close_ns.is_multiple_of(BAR_NS),
            "Pivot bars must be ordered, completed five-minute candles"
        );
        ensure!(
            [high, low, close].iter().all(|x| x.is_finite() && *x > 0.)
                && high >= close
                && close >= low,
            "Invalid Pivot OHLC"
        );
        let open_ns = bar_close_ns - BAR_NS;
        let inside = self.in_session(open_ns)?;
        let day = pivot_session::date(open_ns);
        let new_session = inside && self.session_date != Some(day);
        if new_session {
            if self.settings.session.reset_daily {
                self.reset_session();
            }
            self.session_date = Some(day);
        }
        self.last_bar = bar_close_ns;
        let atr = self.atr.update(high, low, close);
        let mut ph = None;
        let mut pl = None;
        let previous_trend = self.trend;
        if inside || !self.settings.session.reset_daily {
            let n = self.settings.pivot_period;
            self.window.push_back((high, low));
            if self.window.len() > 2 * n + 1 {
                self.window.pop_front();
            }
            if self.window.len() == 2 * n + 1 {
                let (h, l) = self.window[n];
                // Select the rightmost extreme on equal highs/lows. Right-hand bars
                // must all be completed before confirming the candidate at index n.
                if self.window.iter().take(n).all(|x| x.0 <= h)
                    && self.window.iter().skip(n + 1).all(|x| x.0 < h)
                {
                    ph = Some(h);
                }
                if self.window.iter().take(n).all(|x| x.1 >= l)
                    && self.window.iter().skip(n + 1).all(|x| x.1 > l)
                {
                    pl = Some(l);
                }
            }
            if let Some(h) = ph {
                self.resistance = Some(h);
            }
            if let Some(l) = pl {
                self.support = Some(l);
            }
            if let Some(last) = ph.or(pl) {
                self.center = Some(self.center.map_or(last, |c| (2. * c + last) / 3.));
            }
            if let (Some(center), Some(atr)) = (self.center, atr) {
                let basic_up = center - self.settings.atr_factor * atr;
                let basic_down = center + self.settings.atr_factor * atr;
                let next_up = match (self.previous_close, self.up) {
                    (Some(c), Some(up)) if c > up => basic_up.max(up),
                    _ => basic_up,
                };
                let next_down = match (self.previous_close, self.down) {
                    (Some(c), Some(down)) if c < down => basic_down.min(down),
                    _ => basic_down,
                };
                self.trend = if self.down.is_some_and(|v| close > v) {
                    1
                } else if self.up.is_some_and(|v| close < v) {
                    -1
                } else {
                    self.trend
                };
                self.up = Some(next_up);
                self.down = Some(next_down);
            }
            self.previous_close = Some(close);
        }
        // No fresh entry at/after square-off, even when the last bar opened in session.
        let entry_window = inside && self.in_session(bar_close_ns)?;
        let signal = if entry_window && previous_trend == -1 && self.trend == 1 {
            1
        } else if entry_window && previous_trend == 1 && self.trend == -1 {
            -1
        } else {
            0
        };
        Ok(Observation {
            bar_close_ns,
            close,
            atr,
            center: self.center,
            pivot_high: ph,
            pivot_low: pl,
            support: self.support,
            resistance: self.resistance,
            supertrend: if self.trend == 1 { self.up } else { self.down },
            direction: self.trend,
            signal,
            initialized: self.up.is_some() && self.down.is_some(),
            in_session: entry_window,
            new_session,
        })
    }
}

#[cfg(test)]
#[path = "pivot_point_tests.rs"]
mod tests;

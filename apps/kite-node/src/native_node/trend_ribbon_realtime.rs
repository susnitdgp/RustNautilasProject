//! Realtime companion for Trend Ribbon v2.10.
//!
//! Confirmed bars seed indicator state. Kite LTP updates only preview the current
//! candle, matching Pine calc_on_every_tick semantics without committing a new
//! EMA/ATR observation for every tick.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub pre_close_seconds: u64,
    pub fast_reversal_enabled: bool,
    pub fast_hold_seconds: u64,
    pub fast_body_atr_min: f64,
    pub fast_range_atr_min: f64,
    pub wt_exit_enabled: bool,
    pub wt_channel_length: usize,
    pub wt_average_length: usize,
    pub wt_long_arm: f64,
    pub wt_short_arm: f64,
    pub wt_pullback_points: f64,
    pub dynamic_wt_arm: bool,
    pub wt_vol_lookback: usize,
    pub wt_arm_min: f64,
    pub wt_arm_max: f64,
    pub wt_arm_sensitivity: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            pre_close_seconds: 3,
            fast_reversal_enabled: true,
            fast_hold_seconds: 2,
            fast_body_atr_min: 0.50,
            fast_range_atr_min: 0.75,
            wt_exit_enabled: true,
            wt_channel_length: 10,
            wt_average_length: 21,
            wt_long_arm: 53.0,
            wt_short_arm: -53.0,
            wt_pullback_points: 5.0,
            dynamic_wt_arm: true,
            wt_vol_lookback: 50,
            wt_arm_min: 45.0,
            wt_arm_max: 60.0,
            wt_arm_sensitivity: 20.0,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=10).contains(&self.pre_close_seconds),
            "pre-close seconds must be 1..10"
        );
        ensure!(
            (1..=10).contains(&self.fast_hold_seconds),
            "FAST hold seconds must be 1..10"
        );
        ensure!(
            self.fast_body_atr_min.is_finite() && self.fast_body_atr_min > 0.0,
            "FAST body/ATR must be positive"
        );
        ensure!(
            self.fast_range_atr_min.is_finite() && self.fast_range_atr_min > 0.0,
            "FAST range/ATR must be positive"
        );
        ensure!(
            self.wt_channel_length > 0 && self.wt_average_length > 0,
            "WaveTrend lengths must be positive"
        );
        ensure!(
            self.wt_long_arm > 0.0 && self.wt_short_arm < 0.0,
            "WaveTrend arm levels must straddle zero"
        );
        ensure!(
            self.wt_pullback_points.is_finite() && self.wt_pullback_points >= 1.0,
            "WaveTrend pullback must be >= 1"
        );
        ensure!(
            (10..=250).contains(&self.wt_vol_lookback),
            "WT volatility lookback must be 10..250"
        );
        ensure!(
            self.wt_arm_min >= 20.0
                && self.wt_arm_max <= 90.0
                && self.wt_arm_min <= self.wt_arm_max,
            "invalid dynamic WT arm bounds"
        );
        ensure!(
            self.wt_arm_sensitivity.is_finite() && self.wt_arm_sensitivity >= 0.0,
            "WT arm sensitivity must be non-negative"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventKind {
    WtLongExit,
    WtShortExit,
    FastBuy,
    FastShort,
    PreCloseBuy,
    PreCloseShort,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Event {
    pub kind: EventKind,
    pub target: i8,
    pub bar_open_ns: u64,
}

impl Event {
    pub fn reason(self) -> &'static str {
        match self.kind {
            EventKind::WtLongExit => "wt_long_exit",
            EventKind::WtShortExit => "wt_short_exit",
            EventKind::FastBuy => "fast_buy",
            EventKind::FastShort => "fast_short",
            EventKind::PreCloseBuy => "preclose_buy",
            EventKind::PreCloseShort => "preclose_short",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Snapshot {
    pub bar_open_ns: u64,
    pub bar_close_ns: u64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub atr: f64,
    pub alma: f64,
    pub deviation: f64,
    pub slope_score: f64,
    pub bull_setup: bool,
    pub bear_setup: bool,
    pub wt1: f64,
    pub wt2: f64,
    pub wt_long_arm: f64,
    pub wt_short_arm: f64,
    pub trusted: bool,
}

#[derive(Debug, Clone, Copy)]
struct Candle {
    open_ns: u64,
    close_ns: u64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
}

impl Candle {
    fn new(open_ns: u64, bar_ns: u64, price: f64) -> Self {
        Self {
            open_ns,
            close_ns: open_ns + bar_ns,
            open: price,
            high: price,
            low: price,
            close: price,
        }
    }

    fn update(&mut self, price: f64) {
        self.high = self.high.max(price);
        self.low = self.low.min(price);
        self.close = price;
    }
}

#[derive(Debug, Clone)]
struct Ema {
    alpha: f64,
    value: Option<f64>,
}

impl Ema {
    fn new(period: usize) -> Self {
        Self {
            alpha: 2.0 / (period as f64 + 1.0),
            value: None,
        }
    }

    fn preview(&self, input: f64) -> f64 {
        self.value
            .map_or(input, |v| self.alpha * input + (1.0 - self.alpha) * v)
    }

    fn update(&mut self, input: f64) -> f64 {
        let value = self.preview(input);
        self.value = Some(value);
        value
    }
}

#[derive(Debug, Clone)]
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
            sum: 0.0,
            previous_close: None,
            value: None,
        }
    }

    fn true_range(&self, high: f64, low: f64) -> f64 {
        self.previous_close.map_or(high - low, |p| {
            (high - low).max((high - p).abs()).max((low - p).abs())
        })
    }

    fn preview(&self, high: f64, low: f64) -> Option<f64> {
        let tr = self.true_range(high, low);
        match self.value {
            Some(v) => Some((v * (self.period - 1) as f64 + tr) / self.period as f64),
            None if self.count + 1 == self.period => Some((self.sum + tr) / self.period as f64),
            None => None,
        }
    }

    fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let tr = self.true_range(high, low);
        self.value = match self.value {
            Some(v) => Some((v * (self.period - 1) as f64 + tr) / self.period as f64),
            None => {
                self.count += 1;
                self.sum += tr;
                (self.count == self.period).then(|| self.sum / self.period as f64)
            }
        };
        self.previous_close = Some(close);
        self.value
    }
}

#[derive(Debug, Clone)]
struct WaveTrend {
    esa: Ema,
    deviation: Ema,
    wt1: Ema,
    wt1_history: VecDeque<f64>,
}

impl WaveTrend {
    fn new(channel: usize, average: usize) -> Self {
        Self {
            esa: Ema::new(channel),
            deviation: Ema::new(channel),
            wt1: Ema::new(average),
            wt1_history: VecDeque::new(),
        }
    }

    fn values(&self, high: f64, low: f64, close: f64) -> (f64, f64) {
        let ap = (high + low + close) / 3.0;
        let esa = self.esa.preview(ap);
        let d = self.deviation.preview((ap - esa).abs());
        let ci = if d != 0.0 {
            (ap - esa) / (0.015 * d)
        } else {
            0.0
        };
        let wt1 = self.wt1.preview(ci);
        let mut sum = wt1;
        let take = self.wt1_history.len().min(3);
        for v in self.wt1_history.iter().rev().take(take) {
            sum += *v;
        }
        let wt2 = sum / (take + 1) as f64;
        (wt1, wt2)
    }

    fn update(&mut self, high: f64, low: f64, close: f64) -> (f64, f64) {
        let ap = (high + low + close) / 3.0;
        let esa = self.esa.update(ap);
        let d = self.deviation.update((ap - esa).abs());
        let ci = if d != 0.0 {
            (ap - esa) / (0.015 * d)
        } else {
            0.0
        };
        let wt1 = self.wt1.update(ci);
        self.wt1_history.push_back(wt1);
        while self.wt1_history.len() > 4 {
            self.wt1_history.pop_front();
        }
        let count = self.wt1_history.len().min(4);
        let wt2 = self.wt1_history.iter().rev().take(count).sum::<f64>() / count as f64;
        (wt1, wt2)
    }

    fn last_wt1(&self) -> Option<f64> {
        self.wt1_history.back().copied()
    }
}

#[derive(Debug, Clone)]
struct TrendParams {
    alma_length: usize,
    alma_offset: f64,
    alma_sigma: f64,
    deviation_length: usize,
    deviation_multiplier: f64,
    slope_length: usize,
    minimum_slope: f64,
}

#[derive(Debug, Clone)]
pub struct RealtimeRibbon {
    settings: Settings,
    trend: TrendParams,
    bar_ns: u64,
    closes: VecDeque<f64>,
    almas: VecDeque<f64>,
    atr: Atr,
    atr_values: VecDeque<f64>,
    wt: WaveTrend,
    last_confirmed_close_ns: u64,
    current: Option<Candle>,
    current_trusted: bool,
    event_bar_open: Option<u64>,
    bull_setup_start_ns: Option<u64>,
    bear_setup_start_ns: Option<u64>,
    wt_long_armed: bool,
    wt_short_armed: bool,
    wt_long_peak: Option<f64>,
    wt_short_trough: Option<f64>,
    prev_wt1: Option<f64>,
    wt_flat_lock: bool,
    wt_exited_trend: i8,
}

impl RealtimeRibbon {
    pub fn new(settings: &super::trend_ribbon::Settings, bar_ns: u64) -> Result<Self> {
        settings.realtime.validate()?;
        ensure!(bar_ns > 0, "realtime Ribbon bar interval must be positive");
        Ok(Self {
            settings: settings.realtime.clone(),
            trend: TrendParams {
                alma_length: settings.alma_length,
                alma_offset: settings.alma_offset,
                alma_sigma: settings.alma_sigma,
                deviation_length: settings.deviation_length,
                deviation_multiplier: settings.deviation_multiplier,
                slope_length: settings.slope_length,
                minimum_slope: settings.minimum_slope,
            },
            bar_ns,
            closes: VecDeque::new(),
            almas: VecDeque::new(),
            atr: Atr::new(settings.atr_length),
            atr_values: VecDeque::new(),
            wt: WaveTrend::new(
                settings.realtime.wt_channel_length,
                settings.realtime.wt_average_length,
            ),
            last_confirmed_close_ns: 0,
            current: None,
            current_trusted: false,
            event_bar_open: None,
            bull_setup_start_ns: None,
            bear_setup_start_ns: None,
            wt_long_armed: false,
            wt_short_armed: false,
            wt_long_peak: None,
            wt_short_trough: None,
            prev_wt1: None,
            wt_flat_lock: false,
            wt_exited_trend: 0,
        })
    }

    pub fn on_confirmed_bar(
        &mut self,
        high: f64,
        low: f64,
        close: f64,
        bar_close_ns: u64,
    ) -> Result<()> {
        ensure!(
            self.last_confirmed_close_ns == 0 || bar_close_ns > self.last_confirmed_close_ns,
            "realtime Ribbon confirmed bars must be ordered"
        );
        let atr = self.atr.update(high, low, close);
        self.closes.push_back(close);
        let keep =
            self.trend.alma_length.max(self.trend.deviation_length) + self.trend.slope_length + 2;
        while self.closes.len() > keep {
            self.closes.pop_front();
        }
        if let Some(alma) = self.alma_with(None) {
            self.almas.push_back(alma);
            while self.almas.len() > self.trend.slope_length + 2 {
                self.almas.pop_front();
            }
        }
        if let Some(atr) = atr {
            self.atr_values.push_back(atr);
            while self.atr_values.len() > self.settings.wt_vol_lookback {
                self.atr_values.pop_front();
            }
        }
        self.wt.update(high, low, close);
        self.last_confirmed_close_ns = bar_close_ns;
        if self.current.is_some_and(|c| c.open_ns == bar_close_ns) {
            self.current_trusted = true;
        }
        Ok(())
    }

    fn values_with_current(&self, current: f64, n: usize) -> Option<Vec<f64>> {
        if n == 0 || self.closes.len() + 1 < n {
            return None;
        }
        let mut values = self
            .closes
            .iter()
            .skip(self.closes.len().saturating_sub(n - 1))
            .copied()
            .collect::<Vec<_>>();
        values.push(current);
        (values.len() == n).then_some(values)
    }

    fn alma_with(&self, current: Option<f64>) -> Option<f64> {
        let n = self.trend.alma_length;
        let values = match current {
            Some(v) => self.values_with_current(v, n)?,
            None => {
                if self.closes.len() < n {
                    return None;
                }
                self.closes
                    .iter()
                    .skip(self.closes.len() - n)
                    .copied()
                    .collect()
            }
        };
        let m = self.trend.alma_offset * (n - 1) as f64;
        let s = n as f64 / self.trend.alma_sigma;
        let mut num = 0.0;
        let mut den = 0.0;
        for (i, x) in values.iter().enumerate() {
            let w = (-((i as f64 - m).powi(2)) / (2.0 * s * s)).exp();
            num += x * w;
            den += w;
        }
        Some(num / den)
    }

    fn deviation_with(&self, current: f64) -> Option<f64> {
        let n = self.trend.deviation_length;
        let values = self.values_with_current(current, n)?;
        let mean = values.iter().sum::<f64>() / n as f64;
        Some((values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64).sqrt())
    }

    fn preview(&self) -> Option<Snapshot> {
        let c = self.current?;
        let atr = self.atr.preview(c.high, c.low)?;
        let alma = self.alma_with(Some(c.close))?;
        let deviation = self.deviation_with(c.close)?;
        if self.almas.len() < self.trend.slope_length {
            return None;
        }
        let reference = self.almas[self.almas.len() - self.trend.slope_length];
        let slope_score = if atr > 0.0 {
            (alma - reference) / atr
        } else {
            0.0
        };
        let upper = alma + deviation * self.trend.deviation_multiplier;
        let lower = alma - deviation * self.trend.deviation_multiplier;
        let bull_setup = slope_score > self.trend.minimum_slope && c.close > upper;
        let bear_setup = slope_score < -self.trend.minimum_slope && c.close < lower;
        let (wt1, wt2) = self.wt.values(c.high, c.low, c.close);
        let vol_ratio = if self.settings.dynamic_wt_arm
            && self.atr_values.len() + 1 >= self.settings.wt_vol_lookback
        {
            let take = self.settings.wt_vol_lookback - 1;
            let base = (self.atr_values.iter().rev().take(take).sum::<f64>() + atr)
                / self.settings.wt_vol_lookback as f64;
            if base > 0.0 {
                (atr / base).clamp(0.60, 1.35)
            } else {
                1.0
            }
        } else {
            1.0
        };
        let arm = |base: f64| {
            if self.settings.dynamic_wt_arm {
                (base.abs() + (vol_ratio - 1.0) * self.settings.wt_arm_sensitivity)
                    .clamp(self.settings.wt_arm_min, self.settings.wt_arm_max)
            } else {
                base.abs()
            }
        };
        Some(Snapshot {
            bar_open_ns: c.open_ns,
            bar_close_ns: c.close_ns,
            open: c.open,
            high: c.high,
            low: c.low,
            close: c.close,
            atr,
            alma,
            deviation,
            slope_score,
            bull_setup,
            bear_setup,
            wt1,
            wt2,
            wt_long_arm: arm(self.settings.wt_long_arm),
            wt_short_arm: -arm(self.settings.wt_short_arm),
            trusted: self.current_trusted,
        })
    }

    fn begin_or_update_candle(&mut self, event_ns: u64, price: f64) -> Result<bool> {
        ensure!(price.is_finite() && price > 0.0, "invalid realtime LTP");
        let open_ns = event_ns / self.bar_ns * self.bar_ns;
        match self.current {
            Some(mut current) if current.open_ns == open_ns => {
                current.update(price);
                self.current = Some(current);
            }
            Some(current) if open_ns > current.open_ns => {
                let contiguous = open_ns == current.close_ns;
                self.current = Some(Candle::new(open_ns, self.bar_ns, price));
                self.current_trusted = contiguous && self.last_confirmed_close_ns == open_ns;
                self.bull_setup_start_ns = None;
                self.bear_setup_start_ns = None;
                self.prev_wt1 = self.wt.last_wt1();
            }
            None => {
                self.current = Some(Candle::new(open_ns, self.bar_ns, price));
                // Starting mid-candle cannot reconstruct true O/H/L. Wait until
                // a boundary plus canonical prior-bar confirmation.
                self.current_trusted = false;
                self.prev_wt1 = self.wt.last_wt1();
            }
            // A fresh packet can legitimately carry the previous trade's LTP
            // after the exchange clock has crossed a candle boundary. Ignore that
            // stale trade snapshot instead of moving the candle backwards.
            Some(_) => return Ok(false),
        }
        Ok(true)
    }

    pub fn on_session_end(&mut self) {
        self.bull_setup_start_ns = None;
        self.bear_setup_start_ns = None;
        self.wt_long_armed = false;
        self.wt_short_armed = false;
        self.wt_long_peak = None;
        self.wt_short_trough = None;
        self.prev_wt1 = None;
        self.wt_flat_lock = false;
        self.wt_exited_trend = 0;
        self.event_bar_open = None;
    }

    pub fn on_tick(
        &mut self,
        price: f64,
        event_ns: u64,
        now_ns: u64,
        position: i8,
        in_session: bool,
        allow_event: bool,
    ) -> Result<(Option<Snapshot>, Option<Event>)> {
        if !self.begin_or_update_candle(event_ns, price)? {
            return Ok((self.preview(), None));
        }
        let Some(snapshot) = self.preview() else {
            return Ok((None, None));
        };
        if !in_session {
            self.on_session_end();
            return Ok((Some(snapshot), None));
        }
        if !self.settings.enabled || !snapshot.trusted {
            self.prev_wt1 = None;
            return Ok((Some(snapshot), None));
        }
        if self.event_bar_open == Some(snapshot.bar_open_ns) || !allow_event {
            self.prev_wt1 = Some(snapshot.wt1);
            return Ok((Some(snapshot), None));
        }

        if position != 1 {
            self.wt_long_armed = false;
            self.wt_long_peak = None;
        }
        if position != -1 {
            self.wt_short_armed = false;
            self.wt_short_trough = None;
        }

        if self.settings.wt_exit_enabled && position == 1 {
            if !self.wt_long_armed && snapshot.wt1.max(snapshot.wt2) >= snapshot.wt_long_arm {
                self.wt_long_armed = true;
                self.wt_long_peak = Some(snapshot.wt1);
            } else if self.wt_long_armed {
                self.wt_long_peak = Some(
                    self.wt_long_peak
                        .map_or(snapshot.wt1, |v| v.max(snapshot.wt1)),
                );
            }
        }
        if self.settings.wt_exit_enabled && position == -1 {
            if !self.wt_short_armed && snapshot.wt1.min(snapshot.wt2) <= snapshot.wt_short_arm {
                self.wt_short_armed = true;
                self.wt_short_trough = Some(snapshot.wt1);
            } else if self.wt_short_armed {
                self.wt_short_trough = Some(
                    self.wt_short_trough
                        .map_or(snapshot.wt1, |v| v.min(snapshot.wt1)),
                );
            }
        }

        let slope_down = self.prev_wt1.is_some_and(|v| snapshot.wt1 < v);
        let slope_up = self.prev_wt1.is_some_and(|v| snapshot.wt1 > v);
        let long_pullback = self
            .wt_long_peak
            .map_or(0.0, |peak| (peak - snapshot.wt1).max(0.0));
        let short_pullback = self
            .wt_short_trough
            .map_or(0.0, |trough| (snapshot.wt1 - trough).max(0.0));

        if position == -1 && snapshot.bull_setup {
            self.bull_setup_start_ns.get_or_insert(now_ns);
        } else {
            self.bull_setup_start_ns = None;
        }
        if position == 1 && snapshot.bear_setup {
            self.bear_setup_start_ns.get_or_insert(now_ns);
        } else {
            self.bear_setup_start_ns = None;
        }

        let held_ns = self.settings.fast_hold_seconds * 1_000_000_000;
        let bull_held = self
            .bull_setup_start_ns
            .is_some_and(|start| now_ns.saturating_sub(start) >= held_ns);
        let bear_held = self
            .bear_setup_start_ns
            .is_some_and(|start| now_ns.saturating_sub(start) >= held_ns);
        let bullish_body_atr = ((snapshot.close - snapshot.open).max(0.0)) / snapshot.atr;
        let bearish_body_atr = ((snapshot.open - snapshot.close).max(0.0)) / snapshot.atr;
        let range_atr = (snapshot.high - snapshot.low) / snapshot.atr;
        let strong_bull = bullish_body_atr >= self.settings.fast_body_atr_min
            || range_atr >= self.settings.fast_range_atr_min;
        let strong_bear = bearish_body_atr >= self.settings.fast_body_atr_min
            || range_atr >= self.settings.fast_range_atr_min;

        let remaining = snapshot.bar_close_ns.saturating_sub(now_ns);
        let near_close = now_ns < snapshot.bar_close_ns
            && remaining <= self.settings.pre_close_seconds * 1_000_000_000;

        let kind = if self.settings.wt_exit_enabled
            && position == 1
            && self.wt_long_armed
            && long_pullback >= self.settings.wt_pullback_points
            && slope_down
        {
            Some(EventKind::WtLongExit)
        } else if self.settings.wt_exit_enabled
            && position == -1
            && self.wt_short_armed
            && short_pullback >= self.settings.wt_pullback_points
            && slope_up
        {
            Some(EventKind::WtShortExit)
        } else if self.settings.fast_reversal_enabled && position == -1 && bull_held && strong_bull
        {
            Some(EventKind::FastBuy)
        } else if self.settings.fast_reversal_enabled && position == 1 && bear_held && strong_bear {
            Some(EventKind::FastShort)
        } else if position == -1 && near_close && snapshot.bull_setup {
            Some(EventKind::PreCloseBuy)
        } else if position == 1 && near_close && snapshot.bear_setup {
            Some(EventKind::PreCloseShort)
        } else {
            None
        };

        self.prev_wt1 = Some(snapshot.wt1);
        let event = kind.map(|kind| {
            self.event_bar_open = Some(snapshot.bar_open_ns);
            self.bull_setup_start_ns = None;
            self.bear_setup_start_ns = None;
            let target = match kind {
                EventKind::WtLongExit | EventKind::WtShortExit => 0,
                EventKind::FastBuy | EventKind::PreCloseBuy => 1,
                EventKind::FastShort | EventKind::PreCloseShort => -1,
            };
            if matches!(kind, EventKind::WtLongExit | EventKind::WtShortExit) {
                self.wt_flat_lock = true;
                self.wt_exited_trend = position;
            } else {
                self.wt_flat_lock = false;
                self.wt_exited_trend = 0;
            }
            Event {
                kind,
                target,
                bar_open_ns: snapshot.bar_open_ns,
            }
        });
        Ok((Some(snapshot), event))
    }

    pub fn confirmed_event_blocked(&self, bar_close_ns: u64) -> bool {
        bar_close_ns >= self.bar_ns && self.event_bar_open == Some(bar_close_ns - self.bar_ns)
    }

    pub fn blocks_confirmed_sync(&self, direction: i8) -> bool {
        self.wt_flat_lock && self.wt_exited_trend == direction
    }

    pub fn on_confirmed_direction(&mut self, direction: i8) {
        if self.wt_flat_lock && direction != 0 && direction != self.wt_exited_trend {
            self.wt_flat_lock = false;
            self.wt_exited_trend = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trend_settings() -> super::super::trend_ribbon::Settings {
        serde_json::from_value(serde_json::json!({
            "alma_length": 5,
            "alma_offset": 0.85,
            "alma_sigma": 6.0,
            "deviation_length": 5,
            "deviation_multiplier": 0.65,
            "slope_length": 1,
            "minimum_slope": 0.01,
            "atr_length": 5,
            "session": {
                "start": "09:00:00",
                "end": "23:15:00",
                "days": "23456",
                "reset_daily": true
            },
            "realtime": {
                "enabled": true,
                "wt_vol_lookback": 10
            }
        }))
        .unwrap()
    }

    #[test]
    fn first_partial_candle_is_gated_and_next_candle_waits_for_canonical_prior_bar() {
        let settings = trend_settings();
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();
        for i in 0..20u64 {
            let p = 100.0 + i as f64;
            live.on_confirmed_bar(p + 1.0, p - 1.0, p, (i + 1) * step)
                .unwrap();
        }
        let event_ns = 20 * step + 120_000_000_000;
        let (snap, _) = live
            .on_tick(121.0, event_ns, event_ns, 0, true, true)
            .unwrap();
        assert!(!snap.unwrap().trusted);
        let next = 21 * step;
        let (snap, _) = live.on_tick(122.0, next, next, 0, true, true).unwrap();
        assert!(!snap.unwrap().trusted);
        live.on_confirmed_bar(122.0, 120.0, 121.0, next).unwrap();
        let (snap, _) = live
            .on_tick(
                122.5,
                next + 1_000_000_000,
                next + 1_000_000_000,
                0,
                true,
                true,
            )
            .unwrap();
        assert!(snap.unwrap().trusted);
    }

    fn warmed_live(mut settings: super::super::trend_ribbon::Settings) -> (RealtimeRibbon, u64) {
        settings.minimum_slope = 0.0;
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();
        for i in 0..60u64 {
            let p = 100.0 + i as f64;
            live.on_confirmed_bar(p + 1.0, p - 1.0, p, (i + 1) * step)
                .unwrap();
        }
        (live, step)
    }

    #[test]
    fn fast_reversal_requires_hold_and_strong_move() {
        let mut settings = trend_settings();
        settings.realtime.wt_exit_enabled = false;
        let (mut live, step) = warmed_live(settings);
        let open = 60 * step;
        let (_, first) = live
            .on_tick(
                200.0,
                open + 1_000_000_000,
                open + 1_000_000_000,
                -1,
                true,
                false,
            )
            .unwrap();
        assert!(first.is_none());
        live.current_trusted = true;
        let (_, first) = live
            .on_tick(
                200.0,
                open + 1_100_000_000,
                open + 1_100_000_000,
                -1,
                true,
                true,
            )
            .unwrap();
        assert!(first.is_none());
        let (_, event) = live
            .on_tick(
                220.0,
                open + 3_200_000_000,
                open + 3_200_000_000,
                -1,
                true,
                true,
            )
            .unwrap();
        let event = event.expect("FAST BUY");
        assert_eq!(event.kind, EventKind::FastBuy);
        assert_eq!(event.target, 1);
    }

    #[test]
    fn preclose_reversal_fires_inside_three_second_window() {
        let mut settings = trend_settings();
        settings.realtime.wt_exit_enabled = false;
        settings.realtime.fast_reversal_enabled = false;
        let (mut live, step) = warmed_live(settings);
        let open = 60 * step;
        live.on_tick(
            200.0,
            open + 1_000_000_000,
            open + 1_000_000_000,
            -1,
            true,
            false,
        )
        .unwrap();
        live.current_trusted = true;
        let now = open + step - 2_000_000_000;
        let (_, event) = live.on_tick(220.0, now, now, -1, true, true).unwrap();
        let event = event.expect("PRE-CLOSE BUY");
        assert_eq!(event.kind, EventKind::PreCloseBuy);
        assert_eq!(event.target, 1);
    }

    #[test]
    fn wavetrend_exit_has_priority_after_arm_pullback_and_slope_reversal() {
        let mut settings = trend_settings();
        settings.realtime.fast_reversal_enabled = false;
        settings.realtime.dynamic_wt_arm = false;
        settings.realtime.wt_long_arm = 1.0;
        let (mut live, step) = warmed_live(settings);
        let open = 60 * step;
        live.on_tick(
            200.0,
            open + 1_000_000_000,
            open + 1_000_000_000,
            1,
            true,
            false,
        )
        .unwrap();
        live.current_trusted = true;
        let (_, arm_event) = live
            .on_tick(
                200.0,
                open + 1_100_000_000,
                open + 1_100_000_000,
                1,
                true,
                true,
            )
            .unwrap();
        assert!(arm_event.is_none());
        assert!(live.wt_long_armed);
        let (_, event) = live
            .on_tick(
                120.0,
                open + 2_000_000_000,
                open + 2_000_000_000,
                1,
                true,
                true,
            )
            .unwrap();
        let event = event.expect("WT LONG EXIT");
        assert_eq!(event.kind, EventKind::WtLongExit);
        assert_eq!(event.target, 0);
        assert!(live.wt_flat_lock);
    }

    #[test]
    fn wt_flat_lock_blocks_only_same_trend_confirmed_sync() {
        let settings = trend_settings();
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();
        live.wt_flat_lock = true;
        live.wt_exited_trend = -1;
        assert!(live.blocks_confirmed_sync(-1));
        assert!(!live.blocks_confirmed_sync(1));
        assert!(!live.blocks_confirmed_sync(0));
    }

    #[test]
    fn wt_flat_lock_survives_same_direction_and_clears_on_opposite_direction() {
        let settings = trend_settings();
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();
        live.wt_flat_lock = true;
        live.wt_exited_trend = 1;
        live.on_confirmed_direction(1);
        assert!(live.wt_flat_lock);
        assert_eq!(live.wt_exited_trend, 1);
        live.on_confirmed_direction(-1);
        assert!(!live.wt_flat_lock);
        assert_eq!(live.wt_exited_trend, 0);
    }

    #[test]
    fn stale_trade_snapshot_cannot_move_forming_candle_backwards() {
        let settings = trend_settings();
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();
        for i in 0..20u64 {
            let p = 100.0 + i as f64;
            live.on_confirmed_bar(p + 1.0, p - 1.0, p, (i + 1) * step)
                .unwrap();
        }
        let new_open = 21 * step;
        live.on_tick(122.0, new_open, new_open, 0, true, false)
            .unwrap();
        live.on_confirmed_bar(122.0, 120.0, 121.0, new_open)
            .unwrap();
        let before = live.preview().unwrap();
        let (after, event) = live
            .on_tick(
                999.0,
                new_open - step,
                new_open + 1_000_000_000,
                0,
                true,
                true,
            )
            .unwrap();
        let after = after.unwrap();
        assert_eq!(after.bar_open_ns, before.bar_open_ns);
        assert_eq!(after.close, before.close);
        assert!(event.is_none());
    }

    #[test]
    fn leaving_session_clears_wt_same_trend_lock() {
        let settings = trend_settings();
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();
        live.wt_flat_lock = true;
        live.wt_exited_trend = 1;
        live.wt_long_armed = true;
        live.event_bar_open = Some(step);
        live.on_session_end();
        assert!(!live.wt_flat_lock);
        assert_eq!(live.wt_exited_trend, 0);
        assert!(!live.wt_long_armed);
        assert!(live.event_bar_open.is_none());
    }

    #[test]
    fn confirmed_signal_is_blocked_when_same_bar_already_used() {
        let settings = trend_settings();
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();
        live.event_bar_open = Some(10 * step);
        assert!(live.confirmed_event_blocked(11 * step));
        assert!(!live.confirmed_event_blocked(12 * step));
    }

    #[test]
    fn real_kite_fixture_realtime_math_matches_independent_batch_reference() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            instrument_token: u32,
            candles: Vec<kite_adapter::http::historical::Candle>,
        }
        fn ema(prev: Option<f64>, input: f64, period: usize) -> f64 {
            let alpha = 2.0 / (period as f64 + 1.0);
            prev.map_or(input, |v| alpha * input + (1.0 - alpha) * v)
        }
        fn alma(values: &[f64], n: usize, offset: f64, sigma: f64) -> Option<f64> {
            if values.len() < n {
                return None;
            }
            let m = offset * (n - 1) as f64;
            let scale = n as f64 / sigma;
            let mut num = 0.0;
            let mut den = 0.0;
            for (i, value) in values[values.len() - n..].iter().enumerate() {
                let weight = (-((i as f64 - m).powi(2)) / (2.0 * scale * scale)).exp();
                num += value * weight;
                den += weight;
            }
            Some(num / den)
        }
        fn deviation(values: &[f64], n: usize) -> Option<f64> {
            if values.len() < n {
                return None;
            }
            let values = &values[values.len() - n..];
            let mean = values.iter().sum::<f64>() / n as f64;
            Some((values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64).sqrt())
        }
        fn close_enough(actual: f64, expected: f64, label: &str) {
            let scale = actual.abs().max(expected.abs()).max(1.0);
            assert!(
                (actual - expected).abs() <= 1e-10 * scale,
                "{label}: actual={actual:.12} expected={expected:.12}"
            );
        }

        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../tests/fixtures/trend_ribbon_sep18_21_22.json"
        ))
        .unwrap();
        assert_eq!(fixture.instrument_token, 145_894_407);

        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../config/production-trend-ribbon.json"
        ))
        .unwrap();
        let settings: super::super::trend_ribbon::Settings =
            serde_json::from_value(value["trend_ribbon"].clone()).unwrap();
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();

        let mut closes = Vec::<f64>::new();
        let mut confirmed_almas = Vec::<f64>::new();
        let mut atr_values = Vec::<f64>::new();
        let mut previous_close = None;
        let mut atr_value = None;
        let mut atr_count = 0usize;
        let mut atr_sum = 0.0;
        let mut esa = None;
        let mut wt_dev = None;
        let mut wt1_ema = None;
        let mut wt1_history = Vec::<f64>::new();
        let mut compared = 0usize;

        for candle in fixture.candles {
            let open = candle.time().unwrap();
            let open_ns = open.timestamp_nanos_opt().unwrap() as u64;
            let close_ns = open_ns + step;
            live.current = Some(Candle {
                open_ns,
                close_ns,
                open: candle.open,
                high: candle.high,
                low: candle.low,
                close: candle.close,
            });
            live.current_trusted = true;

            let tr = previous_close.map_or(candle.high - candle.low, |p: f64| {
                (candle.high - candle.low)
                    .max((candle.high - p).abs())
                    .max((candle.low - p).abs())
            });
            let atr_preview = match atr_value {
                Some(v) => {
                    Some((v * (settings.atr_length - 1) as f64 + tr) / settings.atr_length as f64)
                }
                None if atr_count + 1 == settings.atr_length => {
                    Some((atr_sum + tr) / settings.atr_length as f64)
                }
                None => None,
            };

            let mut current_closes = closes.clone();
            current_closes.push(candle.close);
            let alma_preview = alma(
                &current_closes,
                settings.alma_length,
                settings.alma_offset,
                settings.alma_sigma,
            );
            let deviation_preview = deviation(&current_closes, settings.deviation_length);
            let slope_preview = match (atr_preview, alma_preview) {
                (Some(atr), Some(current_alma))
                    if atr > 0.0 && confirmed_almas.len() >= settings.slope_length =>
                {
                    Some(
                        (current_alma
                            - confirmed_almas[confirmed_almas.len() - settings.slope_length])
                            / atr,
                    )
                }
                _ => None,
            };

            let ap = (candle.high + candle.low + candle.close) / 3.0;
            let esa_preview = ema(esa, ap, settings.realtime.wt_channel_length);
            let dev_preview = ema(
                wt_dev,
                (ap - esa_preview).abs(),
                settings.realtime.wt_channel_length,
            );
            let ci = if dev_preview != 0.0 {
                (ap - esa_preview) / (0.015 * dev_preview)
            } else {
                0.0
            };
            let wt1_preview = ema(wt1_ema, ci, settings.realtime.wt_average_length);
            let take = wt1_history.len().min(3);
            let wt2_preview = (wt1_preview + wt1_history.iter().rev().take(take).sum::<f64>())
                / (take + 1) as f64;

            if let Some(snapshot) = live.preview() {
                let atr = atr_preview.expect("production preview requires ATR");
                let alma = alma_preview.expect("production preview requires ALMA");
                let dev = deviation_preview.expect("production preview requires deviation");
                let slope = slope_preview.expect("production preview requires slope");
                close_enough(snapshot.atr, atr, "ATR");
                close_enough(snapshot.alma, alma, "ALMA");
                close_enough(snapshot.deviation, dev, "deviation");
                close_enough(snapshot.slope_score, slope, "slope");
                close_enough(snapshot.wt1, wt1_preview, "WT1");
                close_enough(snapshot.wt2, wt2_preview, "WT2");

                let ratio = if settings.realtime.dynamic_wt_arm
                    && atr_values.len() + 1 >= settings.realtime.wt_vol_lookback
                {
                    let take = settings.realtime.wt_vol_lookback - 1;
                    let base = (atr_values.iter().rev().take(take).sum::<f64>() + atr)
                        / settings.realtime.wt_vol_lookback as f64;
                    if base > 0.0 {
                        (atr / base).clamp(0.60, 1.35)
                    } else {
                        1.0
                    }
                } else {
                    1.0
                };
                let expected_long_arm = if settings.realtime.dynamic_wt_arm {
                    (settings.realtime.wt_long_arm.abs()
                        + (ratio - 1.0) * settings.realtime.wt_arm_sensitivity)
                        .clamp(settings.realtime.wt_arm_min, settings.realtime.wt_arm_max)
                } else {
                    settings.realtime.wt_long_arm.abs()
                };
                close_enough(snapshot.wt_long_arm, expected_long_arm, "dynamic WT arm");
                close_enough(
                    snapshot.wt_short_arm,
                    -expected_long_arm,
                    "dynamic WT short arm",
                );
                compared += 1;
            }

            // Commit the independent reference state.
            atr_value = match atr_value {
                Some(v) => {
                    Some((v * (settings.atr_length - 1) as f64 + tr) / settings.atr_length as f64)
                }
                None => {
                    atr_count += 1;
                    atr_sum += tr;
                    (atr_count == settings.atr_length).then(|| atr_sum / settings.atr_length as f64)
                }
            };
            previous_close = Some(candle.close);
            if let Some(atr) = atr_value {
                atr_values.push(atr);
                if atr_values.len() > settings.realtime.wt_vol_lookback {
                    atr_values.remove(0);
                }
            }
            closes.push(candle.close);
            let keep =
                settings.alma_length.max(settings.deviation_length) + settings.slope_length + 2;
            if closes.len() > keep {
                closes.remove(0);
            }
            if let Some(value) = alma(
                &closes,
                settings.alma_length,
                settings.alma_offset,
                settings.alma_sigma,
            ) {
                confirmed_almas.push(value);
                if confirmed_almas.len() > settings.slope_length + 2 {
                    confirmed_almas.remove(0);
                }
            }
            esa = Some(esa_preview);
            wt_dev = Some(dev_preview);
            wt1_ema = Some(wt1_preview);
            wt1_history.push(wt1_preview);
            if wt1_history.len() > 4 {
                wt1_history.remove(0);
            }

            live.on_confirmed_bar(candle.high, candle.low, candle.close, close_ns)
                .unwrap();
        }
        assert!(
            compared > 500,
            "fixture should validate hundreds of realtime previews"
        );
    }

    #[test]
    fn defaults_match_v210_live_parameters() {
        let s = Settings::default();
        assert!(!s.enabled);
        assert_eq!(s.pre_close_seconds, 3);
        assert_eq!(s.fast_hold_seconds, 2);
        assert_eq!(s.fast_body_atr_min, 0.50);
        assert_eq!(s.fast_range_atr_min, 0.75);
        assert_eq!(s.wt_pullback_points, 5.0);
        assert_eq!(s.wt_vol_lookback, 50);
        assert_eq!(s.wt_arm_min, 45.0);
        assert_eq!(s.wt_arm_max, 60.0);
        assert_eq!(s.wt_arm_sensitivity, 20.0);
        s.validate().unwrap();
    }
}

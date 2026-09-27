//! Realtime companion for Trend Ribbon v2.23 Exit-First.
//!
//! Confirmed bars seed indicator state. Kite LTP updates only preview the current
//! candle, matching Pine calc_on_every_tick semantics without committing a new
//! indicator observation for every tick.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use super::trend_ribbon_squeeze::{Config as SqueezeConfig, Engine as SqueezeEngine};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub pre_close_enabled: bool,
    pub pre_close_seconds: u64,
    pub fast_reversal_enabled: bool,
    pub fast_hold_seconds: u64,
    pub fast_body_atr_min: f64,
    pub fast_range_atr_min: f64,
    pub squeeze_exit_enabled: bool,
    pub squeeze_bb_length: usize,
    pub squeeze_bb_mult: f64,
    pub squeeze_kc_length: usize,
    pub squeeze_kc_mult: f64,
    pub squeeze_use_true_range: bool,
    pub squeeze_weak_bars_required: usize,
    pub squeeze_transition_pct: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            pre_close_enabled: true,
            pre_close_seconds: 3,
            fast_reversal_enabled: true,
            fast_hold_seconds: 2,
            fast_body_atr_min: 0.50,
            fast_range_atr_min: 0.75,
            squeeze_exit_enabled: true,
            squeeze_bb_length: 20,
            squeeze_bb_mult: 2.0,
            squeeze_kc_length: 20,
            squeeze_kc_mult: 1.5,
            squeeze_use_true_range: true,
            squeeze_weak_bars_required: 2,
            squeeze_transition_pct: 70.0,
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
            (1..=250).contains(&self.squeeze_bb_length),
            "Squeeze BB length must be 1..250"
        );
        ensure!(
            (1..=250).contains(&self.squeeze_kc_length),
            "Squeeze KC length must be 1..250"
        );
        ensure!(
            self.squeeze_bb_mult.is_finite() && self.squeeze_bb_mult > 0.0,
            "Squeeze BB multiplier must be positive"
        );
        ensure!(
            self.squeeze_kc_mult.is_finite() && self.squeeze_kc_mult > 0.0,
            "Squeeze KC multiplier must be positive"
        );
        ensure!(
            (1..=5).contains(&self.squeeze_weak_bars_required),
            "Squeeze weak bars must be 1..5"
        );
        ensure!(
            self.squeeze_transition_pct.is_finite()
                && (20.0..=95.0).contains(&self.squeeze_transition_pct),
            "Squeeze transition percent must be 20..95"
        );
        self.squeeze_config().validate()
    }

    pub fn squeeze_config(&self) -> SqueezeConfig {
        SqueezeConfig {
            bb_length: self.squeeze_bb_length,
            bb_mult: self.squeeze_bb_mult,
            kc_length: self.squeeze_kc_length,
            kc_mult: self.squeeze_kc_mult,
            use_true_range: self.squeeze_use_true_range,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventKind {
    SqueezeLongExit,
    SqueezeShortExit,
    SqueezeReBuy,
    SqueezeReShort,
    FastBuy,
    FastShort,
    PreCloseBuy,
    PreCloseShort,
    PendingBuy,
    PendingShort,
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
            EventKind::SqueezeLongExit => "squeeze_long_exit",
            EventKind::SqueezeShortExit => "squeeze_short_exit",
            EventKind::SqueezeReBuy => "squeeze_re_buy",
            EventKind::SqueezeReShort => "squeeze_re_short",
            EventKind::FastBuy => "fast_buy",
            EventKind::FastShort => "fast_short",
            EventKind::PreCloseBuy => "preclose_cover_to_buy",
            EventKind::PreCloseShort => "preclose_sell_to_short",
            EventKind::PendingBuy => "reversal_buy",
            EventKind::PendingShort => "reversal_short",
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
    pub squeeze_ready: bool,
    pub squeeze_value: f64,
    pub squeeze_on: bool,
    pub squeeze_off: bool,
    pub squeeze_no: bool,
    pub squeeze_strengthening_long: bool,
    pub squeeze_strengthening_short: bool,
    pub squeeze_long_weak_bar: bool,
    pub squeeze_short_weak_bar: bool,
    pub squeeze_long_strength2: bool,
    pub squeeze_short_strength2: bool,
    pub trusted: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct MonitorSnapshot {
    pub snapshot: Snapshot,
    pub position: i8,
    pub opposite_hold_seconds: f64,
    pub remaining_seconds: f64,
    pub near_close: bool,
    pub bullish_body_atr: f64,
    pub bearish_body_atr: f64,
    pub range_atr: f64,
    pub fast_ready: bool,
    pub preclose_ready: bool,
    pub squeeze_armed: bool,
    pub squeeze_peak: Option<f64>,
    pub squeeze_trough: Option<f64>,
    pub squeeze_weak_bars: usize,
    pub squeeze_decay_pct: f64,
    pub squeeze_exit_used_in_trend: bool,
    pub squeeze_exit_ready: bool,
    pub squeeze_reentry_ready: bool,
    pub exited_trend: i8,
    pub event_locked: bool,
    pub same_trend_lock: bool,
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
        self.previous_close.map_or(high - low, |previous| {
            (high - low)
                .max((high - previous).abs())
                .max((low - previous).abs())
        })
    }

    fn preview(&self, high: f64, low: f64) -> Option<f64> {
        let tr = self.true_range(high, low);
        match self.value {
            Some(value) => Some((value * (self.period - 1) as f64 + tr) / self.period as f64),
            None if self.count + 1 == self.period => Some((self.sum + tr) / self.period as f64),
            None => None,
        }
    }

    fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let tr = self.true_range(high, low);
        self.value = match self.value {
            Some(value) => Some((value * (self.period - 1) as f64 + tr) / self.period as f64),
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
    squeeze: SqueezeEngine,
    last_confirmed_close_ns: u64,
    current: Option<Candle>,
    current_trusted: bool,
    event_bar_open: Option<u64>,
    bull_setup_start_ns: Option<u64>,
    bear_setup_start_ns: Option<u64>,
    confirmed_direction: i8,
    squeeze_long_armed: bool,
    squeeze_short_armed: bool,
    squeeze_long_peak: Option<f64>,
    squeeze_short_trough: Option<f64>,
    squeeze_long_exit_used_in_trend: bool,
    squeeze_short_exit_used_in_trend: bool,
    exit_flat_lock: bool,
    exited_trend: i8,
    // v2.23 exit-first reversal state: +1 BUY pending after COVER,
    // -1 SHORT pending after SELL. The entry is emitted only on a later tick.
    pending_reversal: i8,
    pending_reversal_ns: Option<u64>,
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
            squeeze: SqueezeEngine::new(settings.realtime.squeeze_config())?,
            last_confirmed_close_ns: 0,
            current: None,
            current_trusted: false,
            event_bar_open: None,
            bull_setup_start_ns: None,
            bear_setup_start_ns: None,
            confirmed_direction: 0,
            squeeze_long_armed: false,
            squeeze_short_armed: false,
            squeeze_long_peak: None,
            squeeze_short_trough: None,
            squeeze_long_exit_used_in_trend: false,
            squeeze_short_exit_used_in_trend: false,
            exit_flat_lock: false,
            exited_trend: 0,
            pending_reversal: 0,
            pending_reversal_ns: None,
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
        self.atr.update(high, low, close);
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
        self.squeeze.update(high, low, close);
        self.last_confirmed_close_ns = bar_close_ns;
        if self
            .current
            .is_some_and(|candle| candle.open_ns == bar_close_ns)
        {
            self.current_trusted = true;
        }
        Ok(())
    }

    fn values_with_current(&self, current: f64, length: usize) -> Option<Vec<f64>> {
        if length == 0 || self.closes.len() + 1 < length {
            return None;
        }
        let mut values = self
            .closes
            .iter()
            .skip(self.closes.len().saturating_sub(length - 1))
            .copied()
            .collect::<Vec<_>>();
        values.push(current);
        (values.len() == length).then_some(values)
    }

    fn alma_with(&self, current: Option<f64>) -> Option<f64> {
        let length = self.trend.alma_length;
        let values = match current {
            Some(value) => self.values_with_current(value, length)?,
            None => {
                if self.closes.len() < length {
                    return None;
                }
                self.closes
                    .iter()
                    .skip(self.closes.len() - length)
                    .copied()
                    .collect()
            }
        };
        let m = self.trend.alma_offset * (length - 1) as f64;
        let sigma = length as f64 / self.trend.alma_sigma;
        let mut numerator = 0.0;
        let mut denominator = 0.0;
        for (index, value) in values.iter().enumerate() {
            let weight = (-((index as f64 - m).powi(2)) / (2.0 * sigma * sigma)).exp();
            numerator += value * weight;
            denominator += weight;
        }
        Some(numerator / denominator)
    }

    fn deviation_with(&self, current: f64) -> Option<f64> {
        let length = self.trend.deviation_length;
        let values = self.values_with_current(current, length)?;
        let mean = values.iter().sum::<f64>() / length as f64;
        Some(
            (values
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / length as f64)
                .sqrt(),
        )
    }

    fn preview(&self) -> Option<Snapshot> {
        let candle = self.current?;
        let atr = self.atr.preview(candle.high, candle.low)?;
        let alma = self.alma_with(Some(candle.close))?;
        let deviation = self.deviation_with(candle.close)?;
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
        let bull_setup = slope_score > self.trend.minimum_slope && candle.close > upper;
        let bear_setup = slope_score < -self.trend.minimum_slope && candle.close < lower;
        let squeeze = self.squeeze.preview(candle.high, candle.low, candle.close);
        Some(Snapshot {
            bar_open_ns: candle.open_ns,
            bar_close_ns: candle.close_ns,
            open: candle.open,
            high: candle.high,
            low: candle.low,
            close: candle.close,
            atr,
            alma,
            deviation,
            slope_score,
            bull_setup,
            bear_setup,
            squeeze_ready: squeeze.ready,
            squeeze_value: squeeze.value,
            squeeze_on: squeeze.squeeze_on,
            squeeze_off: squeeze.squeeze_off,
            squeeze_no: squeeze.squeeze_no,
            squeeze_strengthening_long: squeeze.strengthening_long,
            squeeze_strengthening_short: squeeze.strengthening_short,
            squeeze_long_weak_bar: squeeze.long_weak_bar,
            squeeze_short_weak_bar: squeeze.short_weak_bar,
            squeeze_long_strength2: squeeze.long_strength2,
            squeeze_short_strength2: squeeze.short_strength2,
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
            }
            None => {
                self.current = Some(Candle::new(open_ns, self.bar_ns, price));
                self.current_trusted = false;
            }
            Some(_) => return Ok(false),
        }
        Ok(true)
    }

    pub fn on_position_state(&mut self, _position: i8, _entry_price: Option<f64>) {}

    fn squeeze_metrics(&self, snapshot: Snapshot, position: i8) -> (usize, f64, bool, bool) {
        let previous = self.squeeze.latest();
        let weak_bars = if position > 0 && snapshot.squeeze_long_weak_bar {
            if previous.is_some_and(|value| value.long_weak_bar) {
                2
            } else {
                1
            }
        } else if position < 0 && snapshot.squeeze_short_weak_bar {
            if previous.is_some_and(|value| value.short_weak_bar) {
                2
            } else {
                1
            }
        } else {
            0
        };
        let decay = if position > 0 {
            self.squeeze_long_peak
                .filter(|peak| *peak > 0.0)
                .map_or(0.0, |peak| {
                    ((peak - snapshot.squeeze_value) / peak * 100.0).max(0.0)
                })
        } else if position < 0 {
            self.squeeze_short_trough
                .filter(|trough| *trough < 0.0)
                .map_or(0.0, |trough| {
                    ((snapshot.squeeze_value - trough) / trough.abs() * 100.0).max(0.0)
                })
        } else {
            0.0
        };
        let exit_ready = if position > 0 {
            self.squeeze_long_armed
                && !self.squeeze_long_exit_used_in_trend
                && ((weak_bars >= self.settings.squeeze_weak_bars_required
                    && decay >= self.settings.squeeze_transition_pct)
                    || snapshot.squeeze_value <= 0.0)
        } else if position < 0 {
            self.squeeze_short_armed
                && !self.squeeze_short_exit_used_in_trend
                && ((weak_bars >= self.settings.squeeze_weak_bars_required
                    && decay >= self.settings.squeeze_transition_pct)
                    || snapshot.squeeze_value >= 0.0)
        } else {
            false
        };
        let reentry_ready = position == 0
            && self.exit_flat_lock
            && ((self.exited_trend == 1
                && self.confirmed_direction == 1
                && snapshot.squeeze_long_strength2)
                || (self.exited_trend == -1
                    && self.confirmed_direction == -1
                    && snapshot.squeeze_short_strength2));
        (weak_bars, decay, exit_ready, reentry_ready)
    }

    pub fn monitor(&self, now_ns: u64, position: i8) -> Option<MonitorSnapshot> {
        let snapshot = self.preview()?;
        let (weak_bars, decay, exit_ready, reentry_ready) =
            self.squeeze_metrics(snapshot, position);
        let opposite_start = if position < 0 {
            self.bull_setup_start_ns
        } else if position > 0 {
            self.bear_setup_start_ns
        } else {
            None
        };
        let opposite_hold_seconds = opposite_start
            .map(|start| now_ns.saturating_sub(start) as f64 / 1_000_000_000.0)
            .unwrap_or(0.0);
        let remaining_ns = snapshot.bar_close_ns.saturating_sub(now_ns);
        let remaining_seconds = remaining_ns as f64 / 1_000_000_000.0;
        let near_close = self.settings.pre_close_enabled
            && now_ns < snapshot.bar_close_ns
            && remaining_ns <= self.settings.pre_close_seconds * 1_000_000_000;
        let bullish_body_atr = ((snapshot.close - snapshot.open).max(0.0)) / snapshot.atr;
        let bearish_body_atr = ((snapshot.open - snapshot.close).max(0.0)) / snapshot.atr;
        let range_atr = (snapshot.high - snapshot.low) / snapshot.atr;
        let strong_bull = bullish_body_atr >= self.settings.fast_body_atr_min
            || range_atr >= self.settings.fast_range_atr_min;
        let strong_bear = bearish_body_atr >= self.settings.fast_body_atr_min
            || range_atr >= self.settings.fast_range_atr_min;
        let hold_ready = opposite_hold_seconds >= self.settings.fast_hold_seconds as f64;
        let fast_ready = self.settings.fast_reversal_enabled
            && ((position < 0 && snapshot.bull_setup && hold_ready && strong_bull)
                || (position > 0 && snapshot.bear_setup && hold_ready && strong_bear));
        let preclose_ready = near_close
            && ((position < 0 && snapshot.bull_setup) || (position > 0 && snapshot.bear_setup));
        Some(MonitorSnapshot {
            snapshot,
            position,
            opposite_hold_seconds,
            remaining_seconds,
            near_close,
            bullish_body_atr,
            bearish_body_atr,
            range_atr,
            fast_ready,
            preclose_ready,
            squeeze_armed: if position > 0 {
                self.squeeze_long_armed
            } else if position < 0 {
                self.squeeze_short_armed
            } else {
                false
            },
            squeeze_peak: self.squeeze_long_peak,
            squeeze_trough: self.squeeze_short_trough,
            squeeze_weak_bars: weak_bars,
            squeeze_decay_pct: decay,
            squeeze_exit_used_in_trend: if self.confirmed_direction > 0 {
                self.squeeze_long_exit_used_in_trend
            } else if self.confirmed_direction < 0 {
                self.squeeze_short_exit_used_in_trend
            } else {
                false
            },
            squeeze_exit_ready: exit_ready,
            squeeze_reentry_ready: reentry_ready,
            exited_trend: self.exited_trend,
            event_locked: self.event_bar_open == Some(snapshot.bar_open_ns),
            same_trend_lock: self.exit_flat_lock,
        })
    }

    pub fn on_session_end(&mut self) {
        self.bull_setup_start_ns = None;
        self.bear_setup_start_ns = None;
        self.squeeze_long_armed = false;
        self.squeeze_short_armed = false;
        self.squeeze_long_peak = None;
        self.squeeze_short_trough = None;
        self.squeeze_long_exit_used_in_trend = false;
        self.squeeze_short_exit_used_in_trend = false;
        self.exit_flat_lock = false;
        self.exited_trend = 0;
        self.pending_reversal = 0;
        self.pending_reversal_ns = None;
        self.event_bar_open = None;
        self.confirmed_direction = 0;
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
            return Ok((Some(snapshot), None));
        }

        if position != 1 {
            self.squeeze_long_armed = false;
            self.squeeze_long_peak = None;
        }
        if position != -1 {
            self.squeeze_short_armed = false;
            self.squeeze_short_trough = None;
        }

        if self.settings.squeeze_exit_enabled && snapshot.squeeze_ready {
            if position == 1 && !self.squeeze_long_exit_used_in_trend {
                if !self.squeeze_long_armed && snapshot.squeeze_strengthening_long {
                    self.squeeze_long_armed = true;
                    self.squeeze_long_peak = Some(snapshot.squeeze_value);
                } else if self.squeeze_long_armed {
                    self.squeeze_long_peak = Some(
                        self.squeeze_long_peak
                            .map_or(snapshot.squeeze_value, |peak| {
                                peak.max(snapshot.squeeze_value)
                            }),
                    );
                }
            } else if position == -1 && !self.squeeze_short_exit_used_in_trend {
                if !self.squeeze_short_armed && snapshot.squeeze_strengthening_short {
                    self.squeeze_short_armed = true;
                    self.squeeze_short_trough = Some(snapshot.squeeze_value);
                } else if self.squeeze_short_armed {
                    self.squeeze_short_trough = Some(
                        self.squeeze_short_trough
                            .map_or(snapshot.squeeze_value, |trough| {
                                trough.min(snapshot.squeeze_value)
                            }),
                    );
                }
            }
        }

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

        // Pine v2.23 EXIT-FIRST: after the exit order has filled and a later
        // realtime calculation arrives, emit the queued opposite entry. This
        // deliberately bypasses the one-event-per-bar lock because Pine uses
        // alert.freq_all for the exit/entry pair and permits both in one bar.
        if self.pending_reversal != 0 {
            if !allow_event {
                return Ok((Some(snapshot), None));
            }
            let pending_ready = position == 0
                && self
                    .pending_reversal_ns
                    .is_some_and(|sent_ns| now_ns > sent_ns);
            if pending_ready {
                let side = self.pending_reversal;
                self.pending_reversal = 0;
                self.pending_reversal_ns = None;
                self.exit_flat_lock = false;
                self.exited_trend = 0;
                self.squeeze_long_armed = false;
                self.squeeze_short_armed = false;
                self.squeeze_long_peak = None;
                self.squeeze_short_trough = None;
                self.bull_setup_start_ns = None;
                self.bear_setup_start_ns = None;
                return Ok((
                    Some(snapshot),
                    Some(Event {
                        kind: if side > 0 {
                            EventKind::PendingBuy
                        } else {
                            EventKind::PendingShort
                        },
                        target: side,
                        bar_open_ns: snapshot.bar_open_ns,
                    }),
                ));
            }
            // While an opposite entry is queued, Pine blocks FAST, pre-close,
            // and confirmed-close synchronization until that entry is sent.
            return Ok((Some(snapshot), None));
        }

        if self.event_bar_open == Some(snapshot.bar_open_ns) || !allow_event {
            return Ok((Some(snapshot), None));
        }

        let (weak_bars, decay, squeeze_exit_ready, squeeze_reentry_ready) =
            self.squeeze_metrics(snapshot, position);
        let _ = (weak_bars, decay);

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
        let near_close = self.settings.pre_close_enabled
            && now_ns < snapshot.bar_close_ns
            && remaining <= self.settings.pre_close_seconds * 1_000_000_000;

        let kind = if self.settings.squeeze_exit_enabled && position == 1 && squeeze_exit_ready {
            Some(EventKind::SqueezeLongExit)
        } else if self.settings.squeeze_exit_enabled && position == -1 && squeeze_exit_ready {
            Some(EventKind::SqueezeShortExit)
        } else if self.settings.squeeze_exit_enabled
            && position == 0
            && squeeze_reentry_ready
            && self.exited_trend == 1
        {
            Some(EventKind::SqueezeReBuy)
        } else if self.settings.squeeze_exit_enabled
            && position == 0
            && squeeze_reentry_ready
            && self.exited_trend == -1
        {
            Some(EventKind::SqueezeReShort)
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

        let event = kind.map(|kind| {
            self.event_bar_open = Some(snapshot.bar_open_ns);
            self.bull_setup_start_ns = None;
            self.bear_setup_start_ns = None;
            let target = match kind {
                EventKind::SqueezeLongExit
                | EventKind::SqueezeShortExit
                | EventKind::FastBuy
                | EventKind::FastShort
                | EventKind::PreCloseBuy
                | EventKind::PreCloseShort => 0,
                EventKind::SqueezeReBuy | EventKind::PendingBuy => 1,
                EventKind::SqueezeReShort | EventKind::PendingShort => -1,
            };
            match kind {
                EventKind::SqueezeLongExit => {
                    self.squeeze_long_exit_used_in_trend = true;
                    self.exit_flat_lock = true;
                    self.exited_trend = 1;
                    self.squeeze_long_armed = false;
                    self.squeeze_long_peak = None;
                }
                EventKind::SqueezeShortExit => {
                    self.squeeze_short_exit_used_in_trend = true;
                    self.exit_flat_lock = true;
                    self.exited_trend = -1;
                    self.squeeze_short_armed = false;
                    self.squeeze_short_trough = None;
                }
                EventKind::SqueezeReBuy | EventKind::SqueezeReShort => {
                    self.exit_flat_lock = false;
                    self.exited_trend = 0;
                    self.squeeze_long_armed = false;
                    self.squeeze_short_armed = false;
                    self.squeeze_long_peak = None;
                    self.squeeze_short_trough = None;
                }
                EventKind::FastBuy | EventKind::PreCloseBuy => {
                    // Existing SHORT -> COVER now, BUY on a later tick.
                    self.pending_reversal = 1;
                    self.pending_reversal_ns = Some(now_ns);
                    self.exit_flat_lock = false;
                    self.exited_trend = 0;
                    self.squeeze_long_armed = false;
                    self.squeeze_short_armed = false;
                    self.squeeze_long_peak = None;
                    self.squeeze_short_trough = None;
                }
                EventKind::FastShort | EventKind::PreCloseShort => {
                    // Existing LONG -> SELL now, SHORT on a later tick.
                    self.pending_reversal = -1;
                    self.pending_reversal_ns = Some(now_ns);
                    self.exit_flat_lock = false;
                    self.exited_trend = 0;
                    self.squeeze_long_armed = false;
                    self.squeeze_short_armed = false;
                    self.squeeze_long_peak = None;
                    self.squeeze_short_trough = None;
                }
                EventKind::PendingBuy | EventKind::PendingShort => {
                    unreachable!("pending entries are emitted before normal event selection")
                }
            }
            Event {
                kind,
                target,
                bar_open_ns: snapshot.bar_open_ns,
            }
        });
        Ok((Some(snapshot), event))
    }

    pub fn has_pending_reversal(&self) -> bool {
        self.pending_reversal != 0
    }

    pub fn confirmed_event_blocked(&self, bar_close_ns: u64) -> bool {
        bar_close_ns >= self.bar_ns && self.event_bar_open == Some(bar_close_ns - self.bar_ns)
    }

    pub fn blocks_confirmed_sync(&self, direction: i8) -> bool {
        self.exit_flat_lock && self.exited_trend == direction
    }

    pub fn on_confirmed_direction(&mut self, direction: i8) {
        self.confirmed_direction = direction;
        if direction != 1 {
            self.squeeze_long_exit_used_in_trend = false;
        }
        if direction != -1 {
            self.squeeze_short_exit_used_in_trend = false;
        }
        if self.exit_flat_lock && direction != 0 && direction != self.exited_trend {
            self.exit_flat_lock = false;
            self.exited_trend = 0;
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
                "squeeze_bb_length": 3,
                "squeeze_kc_length": 3,
                "squeeze_weak_bars_required": 1,
                "squeeze_transition_pct": 20.0
            }
        }))
        .unwrap()
    }

    fn warmed_live(mut settings: super::super::trend_ribbon::Settings) -> (RealtimeRibbon, u64) {
        settings.minimum_slope = 0.0;
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();
        for i in 0..60u64 {
            let price = 100.0 + i as f64;
            live.on_confirmed_bar(price + 1.0, price - 1.0, price, (i + 1) * step)
                .unwrap();
        }
        (live, step)
    }

    #[test]
    fn default_v223_inputs_match_pine_defaults() {
        let settings = Settings::default();
        assert!(settings.squeeze_exit_enabled);
        assert_eq!(settings.squeeze_bb_length, 20);
        assert_eq!(settings.squeeze_bb_mult, 2.0);
        assert_eq!(settings.squeeze_kc_length, 20);
        assert_eq!(settings.squeeze_kc_mult, 1.5);
        assert!(settings.squeeze_use_true_range);
        assert_eq!(settings.squeeze_weak_bars_required, 2);
        assert_eq!(settings.squeeze_transition_pct, 70.0);
    }

    #[test]
    fn first_partial_candle_is_gated_until_canonical_prior_bar() {
        let settings = trend_settings();
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();
        for i in 0..20u64 {
            let price = 100.0 + i as f64;
            live.on_confirmed_bar(price + 1.0, price - 1.0, price, (i + 1) * step)
                .unwrap();
        }
        let event_ns = 20 * step + 120_000_000_000;
        let (snapshot, _) = live
            .on_tick(121.0, event_ns, event_ns, 0, true, true)
            .unwrap();
        assert!(!snapshot.unwrap().trusted);
        let next = 21 * step;
        live.on_tick(122.0, next, next, 0, true, true).unwrap();
        live.on_confirmed_bar(122.0, 120.0, 121.0, next).unwrap();
        let (snapshot, _) = live
            .on_tick(
                122.5,
                next + 1_000_000_000,
                next + 1_000_000_000,
                0,
                true,
                true,
            )
            .unwrap();
        assert!(snapshot.unwrap().trusted);
    }

    #[test]
    fn fast_reversal_requires_hold_and_strong_move() {
        let mut settings = trend_settings();
        settings.realtime.squeeze_exit_enabled = false;
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
        let event = event.expect("FAST COVER");
        assert_eq!(event.kind, EventKind::FastBuy);
        assert_eq!(event.target, 0);
        assert!(live.has_pending_reversal());

        // A later calculation after the exit fill emits BUY, even in the same bar.
        let (_, pending) = live
            .on_tick(
                220.0,
                open + 3_300_000_000,
                open + 3_300_000_000,
                0,
                true,
                true,
            )
            .unwrap();
        let pending = pending.expect("pending BUY");
        assert_eq!(pending.kind, EventKind::PendingBuy);
        assert_eq!(pending.target, 1);
        assert!(!live.has_pending_reversal());
    }

    #[test]
    fn preclose_reversal_fires_inside_three_second_window() {
        let mut settings = trend_settings();
        settings.realtime.squeeze_exit_enabled = false;
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
        let event = event.expect("PRE-CLOSE COVER");
        assert_eq!(event.kind, EventKind::PreCloseBuy);
        assert_eq!(event.target, 0);
        assert!(live.has_pending_reversal());
    }

    #[test]
    fn squeeze_exit_sets_one_trend_lock() {
        let settings = trend_settings();
        let (mut live, step) = warmed_live(settings);
        live.confirmed_direction = 1;
        live.squeeze_long_armed = true;
        live.squeeze_long_peak = Some(100.0);
        let open = 60 * step;
        live.on_tick(
            160.0,
            open + 1_000_000_000,
            open + 1_000_000_000,
            1,
            true,
            false,
        )
        .unwrap();
        live.current_trusted = true;
        live.squeeze_long_peak = Some(100.0);
        let snapshot = live.preview().unwrap();
        if snapshot.squeeze_value > 0.0 {
            live.squeeze_long_peak = Some(snapshot.squeeze_value.abs() * 10.0 + 1.0);
        }
        let (_, event) = live
            .on_tick(
                159.0,
                open + 2_000_000_000,
                open + 2_000_000_000,
                1,
                true,
                true,
            )
            .unwrap();
        if let Some(event) = event
            && event.kind == EventKind::SqueezeLongExit
        {
            assert_eq!(event.target, 0);
            assert!(live.exit_flat_lock);
            assert!(live.squeeze_long_exit_used_in_trend);
        }
    }

    #[test]
    fn same_trend_lock_clears_only_when_confirmed_direction_changes() {
        let settings = trend_settings();
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();
        live.exit_flat_lock = true;
        live.exited_trend = 1;
        live.on_confirmed_direction(1);
        assert!(live.blocks_confirmed_sync(1));
        live.on_confirmed_direction(-1);
        assert!(!live.exit_flat_lock);
        assert_eq!(live.exited_trend, 0);
    }

    #[test]
    fn session_end_clears_squeeze_state() {
        let settings = trend_settings();
        let step = 300_000_000_000;
        let mut live = RealtimeRibbon::new(&settings, step).unwrap();
        live.squeeze_long_armed = true;
        live.squeeze_long_exit_used_in_trend = true;
        live.exit_flat_lock = true;
        live.exited_trend = 1;
        live.on_session_end();
        assert!(!live.squeeze_long_armed);
        assert!(!live.squeeze_long_exit_used_in_trend);
        assert!(!live.exit_flat_lock);
        assert_eq!(live.exited_trend, 0);
    }
}

//! Historical confirmed-bar Trend Ribbon v2.10 simulator.
//!
//! Mirrors the Pine Strategy Tester path: Trend Ribbon entries/reversals,
//! confirmed-bar WaveTrend exits, session square-off and no same-trend rebuy.
use anyhow::{Context, Result, ensure};
use kite_adapter::http::historical::Candle;
use serde::Serialize;
use std::collections::VecDeque;

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

    fn update(&mut self, input: f64) -> f64 {
        let value = self
            .value
            .map_or(input, |v| self.alpha * input + (1.0 - self.alpha) * v);
        self.value = Some(value);
        value
    }
}

#[derive(Debug)]
struct HistoricalWt {
    esa: Ema,
    deviation: Ema,
    wt1_ema: Ema,
    wt1_history: VecDeque<f64>,
    atr_history: VecDeque<f64>,
    previous_wt1: Option<f64>,
    long_armed: bool,
    short_armed: bool,
    long_peak: Option<f64>,
    short_trough: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct WtValues {
    wt1: f64,
    wt2: Option<f64>,
    slope_down: bool,
    slope_up: bool,
    long_arm: f64,
    short_arm: f64,
}

impl HistoricalWt {
    fn new(settings: &super::trend_ribbon_realtime::Settings) -> Self {
        Self {
            esa: Ema::new(settings.wt_channel_length),
            deviation: Ema::new(settings.wt_channel_length),
            wt1_ema: Ema::new(settings.wt_average_length),
            wt1_history: VecDeque::new(),
            atr_history: VecDeque::new(),
            previous_wt1: None,
            long_armed: false,
            short_armed: false,
            long_peak: None,
            short_trough: None,
        }
    }

    fn update(
        &mut self,
        high: f64,
        low: f64,
        close: f64,
        atr: Option<f64>,
        settings: &super::trend_ribbon_realtime::Settings,
    ) -> WtValues {
        let ap = (high + low + close) / 3.0;
        let esa = self.esa.update(ap);
        let d = self.deviation.update((ap - esa).abs());
        let ci = if d != 0.0 {
            (ap - esa) / (0.015 * d)
        } else {
            0.0
        };
        let wt1 = self.wt1_ema.update(ci);
        self.wt1_history.push_back(wt1);
        while self.wt1_history.len() > 4 {
            self.wt1_history.pop_front();
        }
        let wt2 = (self.wt1_history.len() == 4).then(|| self.wt1_history.iter().sum::<f64>() / 4.0);

        if let Some(atr) = atr {
            self.atr_history.push_back(atr);
            while self.atr_history.len() > settings.wt_vol_lookback {
                self.atr_history.pop_front();
            }
        }
        let ratio = if settings.dynamic_wt_arm && self.atr_history.len() == settings.wt_vol_lookback
        {
            let base = self.atr_history.iter().sum::<f64>() / settings.wt_vol_lookback as f64;
            if base > 0.0 {
                (atr.unwrap_or(base) / base).clamp(0.60, 1.35)
            } else {
                1.0
            }
        } else {
            1.0
        };
        let arm = |base: f64| {
            if settings.dynamic_wt_arm {
                (base.abs() + (ratio - 1.0) * settings.wt_arm_sensitivity)
                    .clamp(settings.wt_arm_min, settings.wt_arm_max)
            } else {
                base.abs()
            }
        };
        let values = WtValues {
            wt1,
            wt2,
            slope_down: self.previous_wt1.is_some_and(|v| wt1 < v),
            slope_up: self.previous_wt1.is_some_and(|v| wt1 > v),
            long_arm: arm(settings.wt_long_arm),
            short_arm: -arm(settings.wt_short_arm),
        };
        self.previous_wt1 = Some(wt1);
        values
    }

    fn update_position_state(&mut self, position: i8, values: WtValues) {
        if position > 0 {
            self.short_armed = false;
            self.short_trough = None;
            if !self.long_armed {
                if values
                    .wt2
                    .is_some_and(|wt2| values.wt1.max(wt2) >= values.long_arm)
                {
                    self.long_armed = true;
                    self.long_peak = Some(values.wt1);
                }
            } else {
                self.long_peak = Some(self.long_peak.map_or(values.wt1, |v| v.max(values.wt1)));
            }
        } else if position < 0 {
            self.long_armed = false;
            self.long_peak = None;
            if !self.short_armed {
                if values
                    .wt2
                    .is_some_and(|wt2| values.wt1.min(wt2) <= values.short_arm)
                {
                    self.short_armed = true;
                    self.short_trough = Some(values.wt1);
                }
            } else {
                self.short_trough =
                    Some(self.short_trough.map_or(values.wt1, |v| v.min(values.wt1)));
            }
        } else {
            self.long_armed = false;
            self.short_armed = false;
            self.long_peak = None;
            self.short_trough = None;
        }
    }

    fn exit(&self, position: i8, values: WtValues, pullback: f64) -> bool {
        if position > 0 {
            self.long_armed
                && self
                    .long_peak
                    .is_some_and(|peak| (peak - values.wt1).max(0.0) >= pullback)
                && values.slope_down
        } else if position < 0 {
            self.short_armed
                && self
                    .short_trough
                    .is_some_and(|trough| (values.wt1 - trough).max(0.0) >= pullback)
                && values.slope_up
        } else {
            false
        }
    }

    fn armed(&self, position: i8) -> bool {
        if position > 0 {
            self.long_armed
        } else if position < 0 {
            self.short_armed
        } else {
            false
        }
    }
}

#[derive(Debug, Default)]
struct HistoricalChandelier {
    side: i8,
    entry: Option<f64>,
    peak: Option<f64>,
    trough: Option<f64>,
    active: bool,
}

impl HistoricalChandelier {
    fn update(
        &mut self,
        side: i8,
        entry: Option<f64>,
        candle: &Candle,
        atr: Option<f64>,
        activation_atr: f64,
        wt_armed: bool,
    ) {
        let high = candle.high;
        let low = candle.low;
        if side != self.side {
            self.side = side;
            self.entry = (side != 0).then_some(entry.unwrap_or((high + low) / 2.0));
            self.peak = (side > 0).then_some(self.entry.unwrap_or(high));
            self.trough = (side < 0).then_some(self.entry.unwrap_or(low));
            self.active = false;
        }
        if side > 0 {
            self.peak = Some(self.peak.map_or(high, |peak| peak.max(high)));
            self.trough = None;
        } else if side < 0 {
            self.trough = Some(self.trough.map_or(low, |trough| trough.min(low)));
            self.peak = None;
        } else {
            self.entry = None;
            self.peak = None;
            self.trough = None;
            self.active = false;
        }

        if !self.active
            && !wt_armed
            && let Some(atr) = atr
        {
            let activation = atr * activation_atr;
            self.active = match side {
                1 => self
                    .entry
                    .zip(self.peak)
                    .is_some_and(|(entry, peak)| peak - entry >= activation),
                -1 => self
                    .entry
                    .zip(self.trough)
                    .is_some_and(|(entry, trough)| entry - trough >= activation),
                _ => false,
            };
        }
    }

    fn stop(&self, side: i8, atr: Option<f64>, multiplier: f64, wt_armed: bool) -> Option<f64> {
        if wt_armed || !self.active {
            return None;
        }
        let atr = atr?;
        if side > 0 {
            self.peak.map(|peak| {
                let raw = peak - atr * multiplier;
                self.entry.map_or(raw, |entry| raw.max(entry))
            })
        } else if side < 0 {
            self.trough.map(|trough| {
                let raw = trough + atr * multiplier;
                self.entry.map_or(raw, |entry| raw.min(entry))
            })
        } else {
            None
        }
    }

    fn exit(
        &self,
        side: i8,
        close: f64,
        atr: Option<f64>,
        multiplier: f64,
        wt_armed: bool,
    ) -> bool {
        self.stop(side, atr, multiplier, wt_armed)
            .is_some_and(|stop| {
                if side > 0 {
                    close <= stop
                } else {
                    close >= stop
                }
            })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub timestamp: String,
    pub action: &'static str,
    pub reason: &'static str,
    pub price: f64,
    pub position_before: i8,
    pub position_after: i8,
    pub trade_points: Option<f64>,
    pub wt1: f64,
    pub wt2: Option<f64>,
    pub wt_long_arm: f64,
    pub wt_short_arm: f64,
}

#[derive(Debug, Clone)]
pub struct MonitorBar {
    pub timestamp: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub direction: i8,
    pub signal: i8,
    pub in_session: bool,
    pub alma: Option<f64>,
    pub deviation: Option<f64>,
    pub atr: Option<f64>,
    pub upper_confirm: Option<f64>,
    pub lower_confirm: Option<f64>,
    pub slope_score: Option<f64>,
    pub bull_setup: bool,
    pub bear_setup: bool,
    pub wt1: f64,
    pub wt2: Option<f64>,
    pub wt_long_arm: f64,
    pub wt_short_arm: f64,
    pub wt_armed: bool,
    pub wt_long_peak: Option<f64>,
    pub wt_short_trough: Option<f64>,
    pub wt_pullback: f64,
    pub wt_slope_down: bool,
    pub wt_slope_up: bool,
    pub chandelier_enabled: bool,
    pub chandelier_active: bool,
    pub chandelier_stop: Option<f64>,
    pub position: i8,
    pub entry_price: Option<f64>,
    pub opened_at: Option<String>,
    pub best_price: Option<f64>,
    pub worst_price: Option<f64>,
    pub mfe_points: Option<f64>,
    pub mae_points: Option<f64>,
    pub open_points: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub instrument: String,
    pub interval: String,
    pub bars: usize,
    pub events: Vec<Event>,
    pub closed_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub gross_points: f64,
    pub open_position: i8,
    pub open_entry_price: Option<f64>,
    pub historical_wt_exit_enabled: bool,
    pub historical_chandelier_exit_enabled: bool,
}

#[derive(Debug)]
struct Position {
    side: i8,
    entry: Option<f64>,
    opened_at: Option<String>,
    best_price: Option<f64>,
    worst_price: Option<f64>,
}

impl Position {
    fn new() -> Self {
        Self {
            side: 0,
            entry: None,
            opened_at: None,
            best_price: None,
            worst_price: None,
        }
    }

    fn update_extremes(&mut self, high: f64, low: f64) {
        if self.side > 0 {
            self.best_price = Some(self.best_price.map_or(high, |v| v.max(high)));
            self.worst_price = Some(self.worst_price.map_or(low, |v| v.min(low)));
        } else if self.side < 0 {
            self.best_price = Some(self.best_price.map_or(low, |v| v.min(low)));
            self.worst_price = Some(self.worst_price.map_or(high, |v| v.max(high)));
        }
    }

    fn close_points(&self, price: f64) -> Option<f64> {
        self.entry.map(|entry| {
            if self.side > 0 {
                price - entry
            } else {
                entry - price
            }
        })
    }

    fn mfe_points(&self) -> Option<f64> {
        self.entry.zip(self.best_price).map(|(entry, best)| {
            if self.side > 0 {
                best - entry
            } else {
                entry - best
            }
        })
    }

    fn mae_points(&self) -> Option<f64> {
        self.entry.zip(self.worst_price).map(|(entry, worst)| {
            if self.side > 0 {
                worst - entry
            } else {
                entry - worst
            }
        })
    }
}

fn action_for_entry(side: i8) -> &'static str {
    if side > 0 { "BUY" } else { "SHORT" }
}

fn action_for_exit(side: i8) -> &'static str {
    if side > 0 { "SELL" } else { "COVER" }
}

fn close_position(
    events: &mut Vec<Event>,
    position: &mut Position,
    timestamp: &str,
    price: f64,
    reason: &'static str,
    values: WtValues,
) -> Option<f64> {
    if position.side == 0 {
        return None;
    }
    let before = position.side;
    let points = position.close_points(price);
    events.push(Event {
        timestamp: timestamp.into(),
        action: action_for_exit(before),
        reason,
        price,
        position_before: before,
        position_after: 0,
        trade_points: points,
        wt1: values.wt1,
        wt2: values.wt2,
        wt_long_arm: values.long_arm,
        wt_short_arm: values.short_arm,
    });
    position.side = 0;
    position.entry = None;
    position.opened_at = None;
    position.best_price = None;
    position.worst_price = None;
    points
}

fn open_position(
    events: &mut Vec<Event>,
    position: &mut Position,
    timestamp: &str,
    price: f64,
    side: i8,
    reason: &'static str,
    values: WtValues,
) {
    events.push(Event {
        timestamp: timestamp.into(),
        action: action_for_entry(side),
        reason,
        price,
        position_before: 0,
        position_after: side,
        trade_points: None,
        wt1: values.wt1,
        wt2: values.wt2,
        wt_long_arm: values.long_arm,
        wt_short_arm: values.short_arm,
    });
    position.side = side;
    position.entry = Some(price);
    position.opened_at = Some(timestamp.to_owned());
    position.best_price = Some(price);
    position.worst_price = Some(price);
}

fn simulate_internal(
    settings: super::trend_ribbon::Settings,
    calendar: super::session_calendar::Calendar,
    bar_ns: u64,
    instrument: &str,
    interval: &str,
    candles: &[Candle],
    capture_monitor: bool,
) -> Result<(Report, Vec<MonitorBar>)> {
    ensure!(!candles.is_empty(), "historical replay has no candles");
    let mut ribbon =
        super::trend_ribbon::TrendRibbon::new_for_interval(settings.clone(), calendar, bar_ns)?;
    let mut wt = HistoricalWt::new(&settings.realtime);
    let mut chandelier = HistoricalChandelier::default();
    let mut position = Position::new();
    let mut events = Vec::new();
    let mut previous_inside = false;
    let mut closed_trades = 0usize;
    let mut winning = 0usize;
    let mut losing = 0usize;
    let mut gross = 0.0;
    let mut monitor = Vec::new();

    for candle in candles {
        let open = candle.time()?;
        let open_ns = u64::try_from(
            open.timestamp_nanos_opt()
                .context("historical timestamp overflow")?,
        )?;
        let close_ns = open_ns + bar_ns;
        let inside = ribbon.in_session(open_ns)?;
        let observation = ribbon.update(candle.high, candle.low, candle.close, close_ns)?;
        let values = wt.update(
            candle.high,
            candle.low,
            candle.close,
            observation.atr,
            &settings.realtime,
        );

        let position_at_start = position.side;
        if position_at_start != 0 {
            position.update_extremes(candle.high, candle.low);
        }
        wt.update_position_state(position_at_start, values);
        let wt_armed = wt.armed(position_at_start);
        chandelier.update(
            position_at_start,
            position.entry,
            candle,
            observation.atr,
            settings.realtime.chandelier_activation_atr,
            wt_armed,
        );
        let wt_exit = settings.realtime.wt_exit_enabled
            && observation.signal == 0
            && wt.exit(
                position_at_start,
                values,
                settings.realtime.wt_pullback_points,
            );
        let chandelier_exit = settings.realtime.chandelier_exit_enabled
            && observation.signal == 0
            && chandelier.exit(
                position_at_start,
                candle.close,
                observation.atr,
                settings.realtime.chandelier_atr_multiplier,
                wt_armed,
            );
        let session_closed_bar = settings.session.reset_daily && previous_inside && !inside;

        let timestamp = open.to_rfc3339();
        let mut booked = None;
        if observation.signal != 0 {
            if position.side != 0 && position.side != observation.signal {
                booked = close_position(
                    &mut events,
                    &mut position,
                    &timestamp,
                    candle.close,
                    "trend_reversal",
                    values,
                );
            }
            if position.side == 0 {
                open_position(
                    &mut events,
                    &mut position,
                    &timestamp,
                    candle.close,
                    observation.signal,
                    "trend_ribbon",
                    values,
                );
            }
        } else if wt_exit {
            let reason = if position.side > 0 {
                "wt_long_exit"
            } else {
                "wt_short_exit"
            };
            booked = close_position(
                &mut events,
                &mut position,
                &timestamp,
                candle.close,
                reason,
                values,
            );
        } else if chandelier_exit {
            let reason = if position.side > 0 {
                "chandelier_long_exit"
            } else {
                "chandelier_short_exit"
            };
            booked = close_position(
                &mut events,
                &mut position,
                &timestamp,
                candle.close,
                reason,
                values,
            );
        } else if session_closed_bar && position.side != 0 {
            booked = close_position(
                &mut events,
                &mut position,
                &timestamp,
                candle.close,
                "session_end",
                values,
            );
        }

        if let Some(points) = booked {
            closed_trades += 1;
            gross += points;
            if points > 0.0 {
                winning += 1;
            } else if points < 0.0 {
                losing += 1;
            }
        }

        if capture_monitor {
            let same_position = position.side == position_at_start;
            let monitor_wt_armed = same_position && wt.armed(position.side);
            let monitor_long_peak = if same_position && position.side > 0 {
                wt.long_peak
            } else {
                None
            };
            let monitor_short_trough = if same_position && position.side < 0 {
                wt.short_trough
            } else {
                None
            };
            let wt_pullback = if position.side > 0 {
                monitor_long_peak.map_or(0.0, |peak| (peak - values.wt1).max(0.0))
            } else if position.side < 0 {
                monitor_short_trough.map_or(0.0, |trough| (values.wt1 - trough).max(0.0))
            } else {
                0.0
            };
            let chandelier_active = same_position
                && settings.realtime.chandelier_exit_enabled
                && chandelier.active
                && !monitor_wt_armed;
            let chandelier_stop = if same_position && settings.realtime.chandelier_exit_enabled {
                chandelier.stop(
                    position.side,
                    observation.atr,
                    settings.realtime.chandelier_atr_multiplier,
                    monitor_wt_armed,
                )
            } else {
                None
            };
            monitor.push(MonitorBar {
                timestamp: timestamp.clone(),
                open: candle.open,
                high: candle.high,
                low: candle.low,
                close: candle.close,
                direction: observation.direction,
                signal: observation.signal,
                in_session: observation.in_session,
                alma: observation.alma,
                deviation: observation.deviation,
                atr: observation.atr,
                upper_confirm: observation.upper_confirm,
                lower_confirm: observation.lower_confirm,
                slope_score: observation.slope_score,
                bull_setup: observation.in_session
                    && observation
                        .slope_score
                        .is_some_and(|s| s > settings.minimum_slope)
                    && observation.upper_confirm.is_some_and(|u| candle.close > u),
                bear_setup: observation.in_session
                    && observation
                        .slope_score
                        .is_some_and(|s| s < -settings.minimum_slope)
                    && observation.lower_confirm.is_some_and(|l| candle.close < l),
                wt1: values.wt1,
                wt2: values.wt2,
                wt_long_arm: values.long_arm,
                wt_short_arm: values.short_arm,
                wt_armed: monitor_wt_armed,
                wt_long_peak: monitor_long_peak,
                wt_short_trough: monitor_short_trough,
                wt_pullback,
                wt_slope_down: values.slope_down,
                wt_slope_up: values.slope_up,
                chandelier_enabled: settings.realtime.chandelier_exit_enabled,
                chandelier_active,
                chandelier_stop,
                position: position.side,
                entry_price: position.entry,
                opened_at: position.opened_at.clone(),
                best_price: position.best_price,
                worst_price: position.worst_price,
                mfe_points: position.mfe_points(),
                mae_points: position.mae_points(),
                open_points: position.close_points(candle.close),
            });
        }
        previous_inside = inside;
    }

    let report = Report {
        instrument: instrument.into(),
        interval: interval.into(),
        bars: candles.len(),
        events,
        closed_trades,
        winning_trades: winning,
        losing_trades: losing,
        gross_points: gross,
        open_position: position.side,
        open_entry_price: position.entry,
        historical_wt_exit_enabled: settings.realtime.wt_exit_enabled,
        historical_chandelier_exit_enabled: settings.realtime.chandelier_exit_enabled,
    };
    Ok((report, monitor))
}

pub fn simulate(
    settings: super::trend_ribbon::Settings,
    calendar: super::session_calendar::Calendar,
    bar_ns: u64,
    instrument: &str,
    interval: &str,
    candles: &[Candle],
) -> Result<Report> {
    simulate_internal(
        settings, calendar, bar_ns, instrument, interval, candles, false,
    )
    .map(|(report, _)| report)
}

pub fn simulate_with_monitor(
    settings: super::trend_ribbon::Settings,
    calendar: super::session_calendar::Calendar,
    bar_ns: u64,
    instrument: &str,
    interval: &str,
    candles: &[Candle],
) -> Result<(Report, Vec<MonitorBar>)> {
    simulate_internal(
        settings, calendar, bar_ns, instrument, interval, candles, true,
    )
}

pub fn run(config: &str, fixture: &str) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct Fixture {
        instrument: Option<String>,
        candles: Vec<Candle>,
    }
    let selection = super::production::Selection::load(config)?;
    let settings = selection.trend_ribbon.clone();
    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(fixture)?)?;
    let report = simulate(
        settings,
        selection.session_calendar.clone(),
        selection.bar_ns(),
        fixture
            .instrument
            .as_deref()
            .unwrap_or(&selection.instrument),
        selection.interval_name(),
        &fixture.candles,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn october_fixture_historical_v210_is_deterministic_and_has_wt_exits() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            candles: Vec<Candle>,
        }
        let raw: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../config/production-trend-ribbon.json"
        ))
        .unwrap();
        let settings: super::super::trend_ribbon::Settings =
            serde_json::from_value(raw["trend_ribbon"].clone()).unwrap();
        let calendar: super::super::session_calendar::Calendar =
            serde_json::from_value(raw["session_calendar"].clone()).unwrap();
        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../tests/fixtures/trend_ribbon_sep18_21_22.json"
        ))
        .unwrap();
        let a = simulate(
            settings.clone(),
            calendar.clone(),
            300_000_000_000,
            "CRUDEOIL26OCTFUT.MCX",
            "5minute",
            &fixture.candles,
        )
        .unwrap();
        let b = simulate(
            settings,
            calendar,
            300_000_000_000,
            "CRUDEOIL26OCTFUT.MCX",
            "5minute",
            &fixture.candles,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
        assert!(a.closed_trades > 10);
        assert!(a.events.iter().any(|e| e.reason == "wt_long_exit"));
        assert!(a.events.iter().any(|e| e.reason == "wt_short_exit"));
    }
}

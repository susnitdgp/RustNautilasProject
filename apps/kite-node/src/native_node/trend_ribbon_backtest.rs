//! Historical confirmed-bar Trend Ribbon v2.22 CLEAN simulator.
//!
//! Mirrors the TradingView Strategy Tester path: Trend Ribbon entries/reversals,
//! Squeeze Momentum transition exits, same-trend Squeeze re-entry, session
//! square-off, and one Squeeze exit maximum per confirmed ribbon trend.
use anyhow::{Context, Result, ensure};
use kite_adapter::http::historical::Candle;
use serde::Serialize;

use super::trend_ribbon_squeeze::{Engine as SqueezeEngine, Values as SqueezeValues};

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub timestamp: String,
    pub action: &'static str,
    pub reason: &'static str,
    pub price: f64,
    pub position_before: i8,
    pub position_after: i8,
    pub trade_points: Option<f64>,
    pub squeeze_value: Option<f64>,
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
    pub squeeze_ready: bool,
    pub squeeze_value: f64,
    pub squeeze_on: bool,
    pub squeeze_off: bool,
    pub squeeze_no: bool,
    pub squeeze_strengthening_long: bool,
    pub squeeze_strengthening_short: bool,
    pub squeeze_armed: bool,
    pub squeeze_peak: Option<f64>,
    pub squeeze_trough: Option<f64>,
    pub squeeze_weak_bars: usize,
    pub squeeze_decay_pct: f64,
    pub squeeze_exit_used_in_trend: bool,
    pub squeeze_exit_ready: bool,
    pub squeeze_reentry_ready: bool,
    pub exited_trend: i8,
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
    pub historical_squeeze_exit_enabled: bool,
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
            self.best_price = Some(self.best_price.map_or(high, |value| value.max(high)));
            self.worst_price = Some(self.worst_price.map_or(low, |value| value.min(low)));
        } else if self.side < 0 {
            self.best_price = Some(self.best_price.map_or(low, |value| value.min(low)));
            self.worst_price = Some(self.worst_price.map_or(high, |value| value.max(high)));
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

#[derive(Debug, Default)]
struct SqueezeTransition {
    long_armed: bool,
    short_armed: bool,
    long_peak: Option<f64>,
    short_trough: Option<f64>,
    long_weak_bars: usize,
    short_weak_bars: usize,
    exited_trend: i8,
    long_exit_used_in_trend: bool,
    short_exit_used_in_trend: bool,
}

impl SqueezeTransition {
    fn on_direction(&mut self, direction: i8) {
        if direction != 1 {
            self.long_exit_used_in_trend = false;
        }
        if direction != -1 {
            self.short_exit_used_in_trend = false;
        }
    }

    fn update_position(&mut self, position: i8, values: SqueezeValues) {
        if position > 0 {
            self.short_armed = false;
            self.short_trough = None;
            self.short_weak_bars = 0;
            if !self.long_armed && values.strengthening_long {
                self.long_armed = true;
                self.long_peak = Some(values.value);
                self.long_weak_bars = 0;
            } else if self.long_armed {
                self.long_peak = Some(
                    self.long_peak
                        .map_or(values.value, |peak| peak.max(values.value)),
                );
                self.long_weak_bars = if values.long_weak_bar {
                    self.long_weak_bars + 1
                } else {
                    0
                };
            }
        } else if position < 0 {
            self.long_armed = false;
            self.long_peak = None;
            self.long_weak_bars = 0;
            if !self.short_armed && values.strengthening_short {
                self.short_armed = true;
                self.short_trough = Some(values.value);
                self.short_weak_bars = 0;
            } else if self.short_armed {
                self.short_trough = Some(
                    self.short_trough
                        .map_or(values.value, |trough| trough.min(values.value)),
                );
                self.short_weak_bars = if values.short_weak_bar {
                    self.short_weak_bars + 1
                } else {
                    0
                };
            }
        } else {
            self.long_armed = false;
            self.short_armed = false;
            self.long_peak = None;
            self.short_trough = None;
            self.long_weak_bars = 0;
            self.short_weak_bars = 0;
        }
    }

    fn decay_pct(&self, position: i8, value: f64) -> f64 {
        if position > 0 {
            self.long_peak
                .filter(|peak| *peak > 0.0)
                .map_or(0.0, |peak| ((peak - value) / peak * 100.0).max(0.0))
        } else if position < 0 {
            self.short_trough
                .filter(|trough| *trough < 0.0)
                .map_or(0.0, |trough| {
                    ((value - trough) / trough.abs() * 100.0).max(0.0)
                })
        } else {
            0.0
        }
    }

    fn exit_ready(
        &self,
        position: i8,
        values: SqueezeValues,
        settings: &super::trend_ribbon_realtime::Settings,
    ) -> bool {
        if position > 0 {
            self.long_armed
                && !self.long_exit_used_in_trend
                && ((self.long_weak_bars >= settings.squeeze_weak_bars_required
                    && self.decay_pct(position, values.value) >= settings.squeeze_transition_pct)
                    || values.value <= 0.0)
        } else if position < 0 {
            self.short_armed
                && !self.short_exit_used_in_trend
                && ((self.short_weak_bars >= settings.squeeze_weak_bars_required
                    && self.decay_pct(position, values.value) >= settings.squeeze_transition_pct)
                    || values.value >= 0.0)
        } else {
            false
        }
    }

    fn mark_exit(&mut self, side: i8) {
        self.exited_trend = side;
        if side > 0 {
            self.long_exit_used_in_trend = true;
            self.long_armed = false;
            self.long_peak = None;
            self.long_weak_bars = 0;
        } else {
            self.short_exit_used_in_trend = true;
            self.short_armed = false;
            self.short_trough = None;
            self.short_weak_bars = 0;
        }
    }

    fn reentry_ready(&self, direction: i8, values: SqueezeValues, in_session: bool) -> bool {
        in_session
            && ((self.exited_trend == 1 && direction == 1 && values.long_strength2)
                || (self.exited_trend == -1 && direction == -1 && values.short_strength2))
    }

    fn mark_reentry(&mut self) {
        self.exited_trend = 0;
        self.long_armed = false;
        self.short_armed = false;
        self.long_peak = None;
        self.short_trough = None;
        self.long_weak_bars = 0;
        self.short_weak_bars = 0;
    }

    fn reset_session(&mut self) {
        *self = Self::default();
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
    squeeze: SqueezeValues,
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
        squeeze_value: squeeze.ready.then_some(squeeze.value),
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
    squeeze: SqueezeValues,
) {
    events.push(Event {
        timestamp: timestamp.into(),
        action: action_for_entry(side),
        reason,
        price,
        position_before: 0,
        position_after: side,
        trade_points: None,
        squeeze_value: squeeze.ready.then_some(squeeze.value),
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
    settings.realtime.validate()?;
    let mut ribbon =
        super::trend_ribbon::TrendRibbon::new_for_interval(settings.clone(), calendar, bar_ns)?;
    let mut squeeze = SqueezeEngine::new(settings.realtime.squeeze_config())?;
    let mut squeeze_state = SqueezeTransition::default();
    let mut position = Position::new();
    let mut events = Vec::new();
    let mut monitor = Vec::new();
    let mut previous_inside = false;
    let mut closed_trades = 0usize;
    let mut winning = 0usize;
    let mut losing = 0usize;
    let mut gross = 0.0;

    for candle in candles {
        let open = candle.time()?;
        let open_ns = u64::try_from(
            open.timestamp_nanos_opt()
                .context("historical timestamp overflow")?,
        )?;
        let close_ns = open_ns + bar_ns;
        let inside = ribbon.in_session(open_ns)?;
        let observation = ribbon.update(candle.high, candle.low, candle.close, close_ns)?;
        let squeeze_values = squeeze.update(candle.high, candle.low, candle.close);

        squeeze_state.on_direction(observation.direction);
        let position_at_start = position.side;
        if position_at_start != 0 {
            position.update_extremes(candle.high, candle.low);
        }
        squeeze_state.update_position(position_at_start, squeeze_values);

        let squeeze_exit_ready = settings.realtime.squeeze_exit_enabled
            && observation.signal == 0
            && squeeze_values.ready
            && squeeze_state.exit_ready(position_at_start, squeeze_values, &settings.realtime);
        let squeeze_reentry_ready = settings.realtime.squeeze_exit_enabled
            && observation.signal == 0
            && position_at_start == 0
            && squeeze_values.ready
            && squeeze_state.reentry_ready(
                observation.direction,
                squeeze_values,
                observation.in_session,
            );
        let session_closed_bar = settings.backtest_square_off
            && settings.session.reset_daily
            && previous_inside
            && !inside;

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
                    squeeze_values,
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
                    squeeze_values,
                );
            }
        } else if squeeze_exit_ready {
            let side = position.side;
            let reason = if side > 0 {
                "squeeze_long_exit"
            } else {
                "squeeze_short_exit"
            };
            booked = close_position(
                &mut events,
                &mut position,
                &timestamp,
                candle.close,
                reason,
                squeeze_values,
            );
            squeeze_state.mark_exit(side);
        } else if squeeze_reentry_ready {
            let side = squeeze_state.exited_trend;
            let reason = if side > 0 {
                "squeeze_re_buy"
            } else {
                "squeeze_re_short"
            };
            open_position(
                &mut events,
                &mut position,
                &timestamp,
                candle.close,
                side,
                reason,
                squeeze_values,
            );
            squeeze_state.mark_reentry();
        } else if session_closed_bar && position.side != 0 {
            booked = close_position(
                &mut events,
                &mut position,
                &timestamp,
                candle.close,
                "session_end",
                squeeze_values,
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
            let active_side = position.side;
            let armed = if active_side > 0 {
                squeeze_state.long_armed
            } else if active_side < 0 {
                squeeze_state.short_armed
            } else {
                false
            };
            let weak_bars = if active_side > 0 {
                squeeze_state.long_weak_bars
            } else if active_side < 0 {
                squeeze_state.short_weak_bars
            } else {
                0
            };
            let decay = squeeze_state.decay_pct(active_side, squeeze_values.value);
            let used = if observation.direction > 0 {
                squeeze_state.long_exit_used_in_trend
            } else if observation.direction < 0 {
                squeeze_state.short_exit_used_in_trend
            } else {
                false
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
                        .is_some_and(|value| value > settings.minimum_slope)
                    && observation
                        .upper_confirm
                        .is_some_and(|upper| candle.close > upper),
                bear_setup: observation.in_session
                    && observation
                        .slope_score
                        .is_some_and(|value| value < -settings.minimum_slope)
                    && observation
                        .lower_confirm
                        .is_some_and(|lower| candle.close < lower),
                squeeze_ready: squeeze_values.ready,
                squeeze_value: squeeze_values.value,
                squeeze_on: squeeze_values.squeeze_on,
                squeeze_off: squeeze_values.squeeze_off,
                squeeze_no: squeeze_values.squeeze_no,
                squeeze_strengthening_long: squeeze_values.strengthening_long,
                squeeze_strengthening_short: squeeze_values.strengthening_short,
                squeeze_armed: armed,
                squeeze_peak: squeeze_state.long_peak,
                squeeze_trough: squeeze_state.short_trough,
                squeeze_weak_bars: weak_bars,
                squeeze_decay_pct: decay,
                squeeze_exit_used_in_trend: used,
                squeeze_exit_ready,
                squeeze_reentry_ready,
                exited_trend: squeeze_state.exited_trend,
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

        if session_closed_bar {
            squeeze_state.reset_session();
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
        historical_squeeze_exit_enabled: settings.realtime.squeeze_exit_enabled,
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
    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(fixture)?)?;
    let report = simulate(
        selection.trend_ribbon.clone(),
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
    fn fixture_v222_is_deterministic() {
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
        let first = simulate(
            settings.clone(),
            calendar.clone(),
            300_000_000_000,
            "CRUDEOIL26OCTFUT.MCX",
            "5minute",
            &fixture.candles,
        )
        .unwrap();
        let second = simulate(
            settings,
            calendar,
            300_000_000_000,
            "CRUDEOIL26OCTFUT.MCX",
            "5minute",
            &fixture.candles,
        )
        .unwrap();
        assert_eq!(first.events.len(), second.events.len());
        assert_eq!(first.gross_points, second.gross_points);
        assert!(first.historical_squeeze_exit_enabled);
    }
}

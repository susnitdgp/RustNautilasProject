//! Historical confirmed-bar simulator for the pure LazyBear SQZ baseline.
use anyhow::{Context, Result, ensure};
use kite_adapter::http::historical::Candle;
use serde::Serialize;

use super::squeeze_momentum_strategy::{Action, Engine, MomentumState, Observation, Settings};

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

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct MonitorBar {
    pub timestamp: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub in_session: bool,
    pub ready: bool,
    pub squeeze_value: f64,
    pub squeeze_on: bool,
    pub squeeze_off: bool,
    pub squeeze_no: bool,
    pub momentum_state: MomentumState,
    pub wave_side: i8,
    pub wave_used: bool,
    pub reentry_armed: bool,
    pub reentries_used: usize,
    pub entry_ready: bool,
    pub strengthening_count: usize,
    pub weakening_count: usize,
    pub retracement_pct: f64,
    pub extreme: Option<f64>,
    pub position: i8,
    pub entry_price: Option<f64>,
    pub opened_at: Option<String>,
    pub best_price: Option<f64>,
    pub worst_price: Option<f64>,
    pub mfe_points: Option<f64>,
    pub mae_points: Option<f64>,
    pub open_points: Option<f64>,
    pub action: Option<Action>,
    pub force_flat_event: bool,
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
    pub force_flat_at_session_end: bool,
}

#[derive(Debug, Clone)]
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

fn open_position(
    events: &mut Vec<Event>,
    position: &mut Position,
    timestamp: &str,
    price: f64,
    action: Action,
    squeeze_value: f64,
) {
    let side = action.target();
    events.push(Event {
        timestamp: timestamp.into(),
        action: action.name(),
        reason: action.reason(),
        price,
        position_before: 0,
        position_after: side,
        trade_points: None,
        squeeze_value: Some(squeeze_value),
    });
    position.side = side;
    position.entry = Some(price);
    position.opened_at = Some(timestamp.to_owned());
    position.best_price = Some(price);
    position.worst_price = Some(price);
}

#[allow(clippy::too_many_arguments)]
fn close_position(
    events: &mut Vec<Event>,
    position: &mut Position,
    timestamp: &str,
    price: f64,
    action: Action,
    squeeze_value: f64,
    force_flat_event: bool,
    exit_zero_cross: bool,
) -> Option<f64> {
    let before = position.side;
    if before == 0 {
        return None;
    }
    let points = position.close_points(price);
    events.push(Event {
        timestamp: timestamp.into(),
        action: action.name(),
        reason: if force_flat_event {
            "session_force_flat"
        } else if exit_zero_cross {
            "zero_cross"
        } else {
            "sqz_transition"
        },
        price,
        position_before: before,
        position_after: 0,
        trade_points: points,
        squeeze_value: Some(squeeze_value),
    });
    position.side = 0;
    position.entry = None;
    position.opened_at = None;
    position.best_price = None;
    position.worst_price = None;
    points
}

fn monitor_bar(
    candle: &Candle,
    timestamp: String,
    observation: Observation,
    position: &Position,
) -> MonitorBar {
    MonitorBar {
        timestamp,
        open: candle.open,
        high: candle.high,
        low: candle.low,
        close: candle.close,
        in_session: observation.in_session,
        ready: observation.ready,
        squeeze_value: observation.value,
        squeeze_on: observation.squeeze_on,
        squeeze_off: observation.squeeze_off,
        squeeze_no: observation.squeeze_no,
        momentum_state: observation.momentum_state,
        wave_side: observation.wave_side,
        wave_used: observation.wave_used,
        reentry_armed: observation.reentry_armed,
        reentries_used: observation.reentries_used,
        entry_ready: observation.entry_ready,
        strengthening_count: observation.strengthening_count,
        weakening_count: observation.weakening_count,
        retracement_pct: observation.retracement_pct,
        extreme: observation.extreme,
        position: position.side,
        entry_price: position.entry,
        opened_at: position.opened_at.clone(),
        best_price: position.best_price,
        worst_price: position.worst_price,
        mfe_points: position.mfe_points(),
        mae_points: position.mae_points(),
        open_points: position.close_points(candle.close),
        action: observation.action,
        force_flat_event: observation.force_flat_event,
    }
}

fn simulate_internal(
    settings: Settings,
    calendar: super::session_calendar::Calendar,
    bar_ns: u64,
    instrument: &str,
    interval: &str,
    candles: &[Candle],
    capture_monitor: bool,
) -> Result<(Report, Vec<MonitorBar>)> {
    ensure!(!candles.is_empty(), "historical replay has no candles");
    let force_flat = settings.force_flat_at_session_end;
    let mut engine = Engine::new(settings, calendar)?;
    let mut position = Position::new();
    let mut events = Vec::new();
    let mut monitor = Vec::new();
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
        if position.side != 0 {
            position.update_extremes(candle.high, candle.low);
        }
        let observation = engine.update_confirmed(
            candle.high,
            candle.low,
            candle.close,
            close_ns,
            bar_ns,
            position.side,
            true,
        )?;
        let timestamp = open.to_rfc3339();
        let mut booked = None;
        if let Some(action) = observation.action {
            match action {
                Action::Buy | Action::Short if position.side == 0 => {
                    open_position(
                        &mut events,
                        &mut position,
                        &timestamp,
                        candle.close,
                        action,
                        observation.value,
                    );
                }
                Action::Sell if position.side > 0 => {
                    booked = close_position(
                        &mut events,
                        &mut position,
                        &timestamp,
                        candle.close,
                        action,
                        observation.value,
                        observation.force_flat_event,
                        observation.exit_zero_cross,
                    );
                }
                Action::Cover if position.side < 0 => {
                    booked = close_position(
                        &mut events,
                        &mut position,
                        &timestamp,
                        candle.close,
                        action,
                        observation.value,
                        observation.force_flat_event,
                        observation.exit_zero_cross,
                    );
                }
                _ => {}
            }
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
            monitor.push(monitor_bar(candle, timestamp, observation, &position));
        }
    }

    Ok((
        Report {
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
            force_flat_at_session_end: force_flat,
        },
        monitor,
    ))
}

pub fn simulate(
    settings: Settings,
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
    settings: Settings,
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
    let instrument = fixture
        .instrument
        .as_deref()
        .unwrap_or(&selection.instrument);
    ensure!(
        instrument == selection.instrument,
        "fixture instrument mismatch"
    );
    let report = simulate(
        selection.squeeze_momentum.clone(),
        selection.session_calendar.clone(),
        selection.bar_ns(),
        &selection.instrument,
        selection.interval_name(),
        &fixture.candles,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

//! Per-slot settings for portfolio instances with `"strategy": "sats"`, read
//! from the slot's own JSON file (`strategy_config`). `params` holds every
//! SATS v1.13.1 input; `execution` decides how the model's events become
//! orders for the configured number of lots.
use anyhow::{Context, Result, ensure};
use kite_adapter::http::historical::{Candle, Interval, KiteInterval};
use sats::{BarInput, Engine, EventKind, Params, SymbolSpec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const STRATEGY: &str = "sats";

/// Used when a config file has no `candle_sources`: each size from the finest
/// Kite interval that divides it.
pub fn default_candle_sources() -> BTreeMap<String, KiteInterval> {
    [
        (1, KiteInterval::Minute),
        (2, KiteInterval::Minute),
        (3, KiteInterval::ThreeMinute),
        (5, KiteInterval::FiveMinute),
        (6, KiteInterval::ThreeMinute),
        (10, KiteInterval::TenMinute),
        (15, KiteInterval::FifteenMinute),
        (30, KiteInterval::ThirtyMinute),
    ]
    .into_iter()
    .map(|(m, k)| (m.to_string(), k))
    .collect()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SatsConfig {
    /// Must be "sats"; guards against pointing a slot at another strategy's file.
    pub strategy: String,
    /// Candle size the engine runs on, in minutes (also drives preset "Auto").
    /// Must have an entry in `candle_sources`.
    pub bar_minutes: u32,
    /// Candle minutes → Kite historical interval its warm-up/backtest history is
    /// fetched at, e.g. {"2": "minute"} builds 2m candles from 1m history. Kite serves
    /// "minute", "3minute", "5minute", "10minute", "15minute", "30minute". Live bars
    /// are always built from WebSocket ticks at `bar_minutes`.
    #[serde(default = "default_candle_sources")]
    pub candle_sources: BTreeMap<String, KiteInterval>,
    /// Instrument tick (`syminfo.mintick`), e.g. 1.0 for CRUDEOILM.
    pub tick_size: f64,
    pub lots: u32,
    /// Rupees per 1.0 price move for one lot (CRUDEOILM 10).
    pub point_value: f64,
    /// Live-runner settings (calendar, daily square-off, product).
    pub live: LiveSettings,
    pub execution: Execution,
    pub params: Params,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LiveSettings {
    /// Reviewed exchange session calendar JSON (holidays, special sessions).
    pub session_calendar: String,
    /// IST time of the daily square-off; no new entries from this bar onwards.
    pub square_off: chrono::NaiveTime,
    /// Broker product; the production client only accepts MIS.
    pub product: String,
}

impl LiveSettings {
    pub fn square_off_minute(&self) -> u32 {
        use chrono::Timelike;
        self.square_off.hour() * 60 + self.square_off.minute()
    }
}

/// How model events map to orders.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    /// "thirds": ⅓ of the lots at TP1, ⅓ at TP2, rest at TP3 (lots must divide by 3).
    /// "single": the whole position exits at `single_exit_at` (or SL / flip / timeout first).
    /// "trail":  no profit target. The stop is checked on every tick; at TP1 it
    ///           moves to breakeven and then trails on the SuperTrend line. Exits
    ///           on the stop, a trend flip, a timeout or the square-off.
    pub exit_mode: ExitMode,
    pub single_exit_at: Target,
    /// Settings for `exit_mode: "trail"` (ignored otherwise).
    #[serde(default)]
    pub trail: TrailSettings,
    /// Backtest friction per lot per round trip, in price points.
    pub round_trip_cost_points: f64,
    /// Backtest adverse slippage per order, in price points.
    pub slippage_points_per_side: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExitMode {
    Thirds,
    Single,
    Trail,
}

/// Trailing-stop rules for `exit_mode: "trail"`. Levels only ever move in the
/// trade's favour.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TrailSettings {
    /// At TP1 the stop moves to breakeven = actual entry fill ± this many points
    /// in the trade's favour (e.g. 2.0 to cover costs; 0.0 = exact entry).
    pub breakeven_offset_points: f64,
    /// After TP1, also trail the stop on the SATS SuperTrend line at each bar
    /// close (the tighter of breakeven and the line is used).
    pub supertrend_trail: bool,
}

impl Default for TrailSettings {
    fn default() -> Self {
        Self { breakeven_offset_points: 0.0, supertrend_trail: true }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Target {
    #[serde(rename = "TP1")]
    Tp1,
    #[serde(rename = "TP2")]
    Tp2,
    #[serde(rename = "TP3")]
    Tp3,
}

impl Execution {
    /// Lots to close for a model exit event, given the lots still open.
    /// Entries are not handled here (they always open `lots`).
    pub fn lots_to_close(&self, kind: EventKind, lots: u32, open_lots: u32) -> u32 {
        if open_lots == 0 {
            return 0;
        }
        let closing = matches!(kind, EventKind::SlHit | EventKind::FlipExit | EventKind::TimeoutExit | EventKind::Tp3Hit);
        match self.exit_mode {
            ExitMode::Thirds => match kind {
                EventKind::Tp1Hit | EventKind::Tp2Hit => (lots / 3).min(open_lots),
                _ if closing => open_lots,
                _ => 0,
            },
            ExitMode::Single => {
                let target = match self.single_exit_at {
                    Target::Tp1 => EventKind::Tp1Hit,
                    Target::Tp2 => EventKind::Tp2Hit,
                    Target::Tp3 => EventKind::Tp3Hit,
                };
                if closing || kind == target { open_lots } else { 0 }
            }
            // No targets: TP hits only move the trailing stop. SATS's SL, flip and
            // timeout still close what is open; its TP3 does not (the position keeps
            // trailing after SATS's own trade has ended).
            ExitMode::Trail => {
                if matches!(kind, EventKind::SlHit | EventKind::FlipExit | EventKind::TimeoutExit) { open_lots } else { 0 }
            }
        }
    }
}

impl SatsConfig {
    pub fn load(path: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("Cannot read sats strategy config {path}"))?;
        let config: Self =
            serde_json::from_str(&text).with_context(|| format!("Invalid sats strategy config {path}"))?;
        config.validate().with_context(|| format!("Rejected sats strategy config {path}"))?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.strategy == STRATEGY, "strategy must be \"{STRATEGY}\"");
        for (minutes, source) in &self.candle_sources {
            let m: u32 = minutes.parse().map_err(|_| anyhow::anyhow!("candle_sources key \"{minutes}\" is not a whole number of minutes"))?;
            Interval::built_from(m, *source).with_context(|| format!("candle_sources \"{minutes}\""))?;
        }
        self.interval()?;
        ensure!(self.tick_size.is_finite() && self.tick_size > 0.0, "tick_size must be positive");
        ensure!((1..=100).contains(&self.lots), "lots must be 1..100");
        ensure!(self.point_value.is_finite() && self.point_value > 0.0, "point_value must be positive");
        let e = &self.execution;
        ensure!(
            [e.round_trip_cost_points, e.slippage_points_per_side].iter().all(|v| v.is_finite() && *v >= 0.0),
            "execution costs must be non-negative"
        );
        ensure!(
            e.exit_mode != ExitMode::Thirds || self.lots.is_multiple_of(3),
            "exit_mode \"thirds\" needs lots divisible by 3 (use \"single\" for {} lot(s))",
            self.lots
        );
        if e.exit_mode == ExitMode::Trail {
            ensure!(
                e.trail.breakeven_offset_points.is_finite() && e.trail.breakeven_offset_points >= 0.0,
                "trail.breakeven_offset_points must be zero or positive"
            );
        }
        ensure!(self.live.product == "MIS", "live.product must be MIS (intraday)");
        ensure!(
            chrono::Timelike::second(&self.live.square_off) == 0
                && chrono::Timelike::minute(&self.live.square_off).is_multiple_of(self.bar_minutes),
            "live.square_off must fall on a {}-minute bar boundary",
            self.bar_minutes
        );
        self.params.validate().map_err(anyhow::Error::msg)?;
        Ok(())
    }

    /// Candle size plus the Kite history interval it is built from (`candle_sources`).
    pub fn interval(&self) -> Result<Interval> {
        let source = self.candle_sources.get(&self.bar_minutes.to_string()).ok_or_else(|| {
            anyhow::anyhow!(
                "bar_minutes {} has no entry in candle_sources (configured: {})",
                self.bar_minutes,
                self.candle_sources.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;
        Interval::built_from(self.bar_minutes, *source)
    }

    pub fn bar_ns(&self) -> i64 {
        i64::from(self.bar_minutes) * 60_000_000_000
    }

    pub fn engine(&self) -> Result<Engine> {
        Engine::new(
            self.params.clone(),
            SymbolSpec { tick_size: self.tick_size, bar_minutes: f64::from(self.bar_minutes) },
        )
        .map_err(anyhow::Error::msg)
    }

    /// Kite candle (timestamped at its open, IST) -> engine bar.
    pub fn bar(&self, candle: &Candle) -> Result<BarInput> {
        let open = candle
            .time()?
            .timestamp_nanos_opt()
            .ok_or_else(|| anyhow::anyhow!("Candle timestamp overflow"))?;
        Ok(BarInput {
            open_time_ns: open,
            close_time_ns: open + self.bar_ns(),
            open: candle.open,
            high: candle.high,
            low: candle.low,
            close: candle.close,
            volume: Some(candle.volume as f64),
        })
    }

    /// Engine primed on completed history; events during warm-up are discarded.
    #[allow(dead_code)] // used by the live runner once a slot is wired to a node
    pub fn warm_engine(&self, candles: &[Candle]) -> Result<Engine> {
        let mut engine = self.engine()?;
        for candle in candles {
            engine.on_bar(&self.bar(candle)?);
        }
        Ok(engine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIPPED: &str = include_str!("../../../../config/sats-crudeoilm.json");

    #[test]
    fn shipped_config_is_valid_and_spells_out_every_param() {
        let c: SatsConfig = serde_json::from_str(SHIPPED).unwrap();
        c.validate().unwrap();
        let expected = Params { tp_mode: sats::TpMode::Dynamic, ..Params::default() };
        assert_eq!(c.params, expected, "script defaults except Dynamic TP mode");
        let raw: serde_json::Value = serde_json::from_str(SHIPPED).unwrap();
        let written: Vec<_> = raw["params"].as_object().unwrap().keys().cloned().collect();
        let all = serde_json::to_value(Params::default()).unwrap();
        for key in all.as_object().unwrap().keys() {
            assert!(written.contains(key), "param {key} missing from config/sats-crudeoilm.json");
        }
    }

    fn exec(mode: ExitMode, at: Target) -> Execution {
        Execution {
            exit_mode: mode,
            single_exit_at: at,
            trail: TrailSettings::default(),
            round_trip_cost_points: 0.0,
            slippage_points_per_side: 0.0,
        }
    }

    #[test]
    fn trail_mode_targets_only_move_the_stop() {
        let e = exec(ExitMode::Trail, Target::Tp1);
        for kind in [EventKind::Tp1Hit, EventKind::Tp2Hit, EventKind::Tp3Hit] {
            assert_eq!(e.lots_to_close(kind, 1, 1), 0, "{kind:?}");
        }
        for kind in [EventKind::SlHit, EventKind::FlipExit, EventKind::TimeoutExit] {
            assert_eq!(e.lots_to_close(kind, 1, 1), 1, "{kind:?}");
        }
        let mut c: SatsConfig = serde_json::from_str(SHIPPED).unwrap();
        c.execution.exit_mode = ExitMode::Trail;
        c.validate().unwrap();
        c.execution.trail.breakeven_offset_points = -1.0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn single_mode_exits_everything_at_the_chosen_target() {
        let e = exec(ExitMode::Single, Target::Tp2);
        assert_eq!(e.lots_to_close(EventKind::Tp1Hit, 1, 1), 0);
        assert_eq!(e.lots_to_close(EventKind::Tp2Hit, 1, 1), 1);
        assert_eq!(e.lots_to_close(EventKind::Tp3Hit, 1, 0), 0, "already flat");
        assert_eq!(e.lots_to_close(EventKind::SlHit, 1, 1), 1);
        assert_eq!(e.lots_to_close(EventKind::FlipExit, 1, 1), 1);
    }

    #[test]
    fn thirds_mode_scales_out() {
        let e = exec(ExitMode::Thirds, Target::Tp3);
        assert_eq!(e.lots_to_close(EventKind::Tp1Hit, 6, 6), 2);
        assert_eq!(e.lots_to_close(EventKind::Tp2Hit, 6, 4), 2);
        assert_eq!(e.lots_to_close(EventKind::Tp3Hit, 6, 2), 2);
        assert_eq!(e.lots_to_close(EventKind::TimeoutExit, 6, 4), 4);
    }

    #[test]
    fn thirds_with_one_lot_is_rejected() {
        let mut c: SatsConfig = serde_json::from_str(SHIPPED).unwrap();
        c.execution.exit_mode = ExitMode::Thirds;
        assert!(c.validate().is_err());
        c.lots = 3;
        c.validate().unwrap();
        c.strategy = "vce-mojo".into();
        assert!(c.validate().is_err());
    }
}

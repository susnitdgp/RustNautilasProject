//! Per-slot settings for portfolio instances with `"strategy": "sats"`, read
//! from the slot's own JSON file (`strategy_config`). `params` holds every
//! SATS v1.13.1 input; `execution` decides how the model's events become
//! orders for the configured number of lots.
use anyhow::{Context, Result, ensure};
use kite_adapter::http::historical::{Candle, Interval};
use sats::{BarInput, Engine, EventKind, Params, SymbolSpec};
use serde::{Deserialize, Serialize};

pub const STRATEGY: &str = "sats";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SatsConfig {
    /// Must be "sats"; guards against pointing a slot at another strategy's file.
    pub strategy: String,
    /// Candle size the engine runs on: 1, 3 or 5 minutes (also drives preset "Auto").
    pub bar_minutes: u32,
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
    pub exit_mode: ExitMode,
    pub single_exit_at: Target,
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
        ensure!(matches!(self.bar_minutes, 1 | 3 | 5), "bar_minutes must be 1, 3 or 5");
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

    pub fn interval(&self) -> Interval {
        match self.bar_minutes {
            1 => Interval::OneMinute,
            3 => Interval::ThreeMinute,
            _ => Interval::FiveMinute,
        }
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
        Execution { exit_mode: mode, single_exit_at: at, round_trip_cost_points: 0.0, slippage_points_per_side: 0.0 }
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

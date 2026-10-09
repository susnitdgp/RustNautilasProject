//! Per-slot settings for portfolio instances with `"strategy": "vce-mojo"`.
//! Each instance points at its own file through `strategy_config`, so every
//! asset (MCX or NFO) carries its own bar size, lot count, point value, EOD
//! cut-off and cost model while sharing one engine implementation.
use anyhow::{Context, Result, ensure};
use kite_adapter::http::historical::{Candle, Interval};
use serde::{Deserialize, Serialize};
use vce_mojo::{BarInput, Engine, Params};

pub const STRATEGY: &str = "vce-mojo";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VceConfig {
    /// Must be "vce-mojo"; guards against pointing a slot at another strategy's file.
    pub strategy: String,
    /// Candle size the engine runs on: 1, 3 or 5 minutes.
    pub bar_minutes: u32,
    #[serde(default = "one_lot")]
    pub lots: u32,
    /// Rupees per 1.0 price move for one lot (e.g. CRUDEOIL 100, CRUDEOILM 10).
    pub point_value: f64,
    #[serde(default)]
    pub params: Params,
    #[serde(default)]
    pub costs: Costs,
}

/// Backtest-only friction, in price points.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Costs {
    pub round_trip_points: f64,
    pub slippage_points_per_side: f64,
}

impl Default for Costs {
    fn default() -> Self {
        Self {
            round_trip_points: 2.0,
            slippage_points_per_side: 0.5,
        }
    }
}

const fn one_lot() -> u32 {
    1
}

impl VceConfig {
    pub fn load(path: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read vce-mojo strategy config {path}"))?;
        let config: Self = serde_json::from_str(&text)
            .with_context(|| format!("Invalid vce-mojo strategy config {path}"))?;
        config.validate().with_context(|| format!("Rejected vce-mojo strategy config {path}"))?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.strategy == STRATEGY, "strategy must be \"{STRATEGY}\"");
        ensure!(
            matches!(self.bar_minutes, 1 | 3 | 5),
            "bar_minutes must be 1, 3 or 5"
        );
        ensure!((1..=100).contains(&self.lots), "lots must be 1..100");
        ensure!(
            self.point_value.is_finite() && self.point_value > 0.0,
            "point_value must be positive"
        );
        ensure!(
            [self.costs.round_trip_points, self.costs.slippage_points_per_side]
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0),
            "costs must be non-negative"
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
        Engine::new(self.params.clone()).map_err(anyhow::Error::msg)
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
        })
    }

    /// Engine primed on completed history (signals during warm-up are discarded).
    /// Used by the live runner when a slot is wired to a node.
    #[allow(dead_code)]
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

    #[test]
    fn shipped_examples_are_valid() {
        for text in [
            include_str!("../../../../config/vce-mojo-crudeoil.example.json"),
            include_str!("../../../../config/vce-mojo-gold.example.json"),
        ] {
            let c: VceConfig = serde_json::from_str(text).unwrap();
            c.validate().unwrap();
        }
    }

    #[test]
    fn rejects_wrong_strategy_and_bad_values() {
        let base = || VceConfig {
            strategy: STRATEGY.into(),
            bar_minutes: 1,
            lots: 1,
            point_value: 100.0,
            params: Params::default(),
            costs: Costs::default(),
        };
        base().validate().unwrap();
        assert!(VceConfig { strategy: "ilrc".into(), ..base() }.validate().is_err());
        assert!(VceConfig { bar_minutes: 15, ..base() }.validate().is_err());
        assert!(VceConfig { point_value: 0.0, ..base() }.validate().is_err());
        let mut p = base();
        p.params.tp3_r = 20.0;
        assert!(p.validate().is_err());
    }

    #[test]
    fn candle_maps_to_open_and_close_times() {
        let c = Candle {
            timestamp: "2026-10-08T09:01:00+0530".into(),
            open: 1.0,
            high: 2.0,
            low: 0.5,
            close: 1.5,
            volume: 0,
            oi: 0,
        };
        let cfg = VceConfig {
            strategy: STRATEGY.into(),
            bar_minutes: 3,
            lots: 1,
            point_value: 1.0,
            params: Params::default(),
            costs: Costs::default(),
        };
        let b = cfg.bar(&c).unwrap();
        assert_eq!(b.close_time_ns - b.open_time_ns, 180_000_000_000);
        assert_eq!(b.open_time_ns, c.time().unwrap().timestamp_nanos_opt().unwrap());
    }
}

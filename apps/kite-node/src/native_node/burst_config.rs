//! CRUDE-BURST slot settings (JSON): contract, size, costs and the rule params.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub const STRATEGY: &str = "burst";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BurstConfig {
    /// Must be "burst".
    pub strategy: String,
    /// Kite trading symbol and instrument token of the contract traded / replayed.
    pub symbol: String,
    pub instrument_token: u32,
    pub lots: u32,
    /// Rupees per 1.0 price move for one lot (CRUDEOILM 10, CRUDEOIL 100).
    pub point_value: f64,
    /// All-in cost of one round trip for one lot, in points (brokerage, CTT,
    /// exchange, GST, stamp). CRUDEOILM ≈ ₹60 ≈ 6 points.
    pub round_trip_cost_points: f64,
    /// Adverse slippage per market fill, points.
    pub slippage_points_per_side: f64,
    /// IST time: open positions are closed at the first bar close at/after it and
    /// no entries are taken from it on.
    pub square_off: chrono::NaiveTime,
    #[serde(default)]
    pub params: burst::Params,
}

impl BurstConfig {
    pub fn load(path: &str) -> Result<Self> {
        let raw = std::fs::read(path).with_context(|| format!("Cannot read {path}"))?;
        let config: Self = serde_json::from_slice(&raw).with_context(|| format!("Invalid burst config {path}"))?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.strategy == STRATEGY, "strategy must be \"{STRATEGY}\"");
        ensure!(self.instrument_token > 0, "instrument_token required");
        ensure!((1..=100).contains(&self.lots), "lots must be 1..100");
        ensure!(self.point_value.is_finite() && self.point_value > 0.0, "point_value must be positive");
        ensure!(
            [self.round_trip_cost_points, self.slippage_points_per_side].iter().all(|v| v.is_finite() && *v >= 0.0),
            "costs must be zero or positive"
        );
        self.params.validate().map_err(anyhow::Error::msg)?;
        ensure!(self.params.entry_to <= self.square_off, "params.entry_to must not be after square_off");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SHIPPED: &str = include_str!("../../../../config/burst-crudeoilm.json");
    #[test]
    fn shipped_config_is_valid() {
        let c: BurstConfig = serde_json::from_str(SHIPPED).unwrap();
        c.validate().unwrap();
        assert_eq!(c.symbol, "CRUDEOILM26OCTFUT");
        let mut bad = c.clone();
        bad.strategy = "sats".into();
        assert!(bad.validate().is_err());
    }
}

//! Validated production selection and offline release verification; no live activation.
use anyhow::{Result, ensure};
use serde::Deserialize;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    strategy: String,
    instrument: String,
    interval: String,
    contracts: u32,
    supertrend_period: u32,
    supertrend_multiplier: u32,
    macd_fast: u32,
    macd_slow: u32,
    macd_signal: u32,
    session_vwap: bool,
    atr_stop_enabled: bool,
    live_orders_enabled: bool,
}
impl Selection {
    pub fn load(path: &str) -> Result<Self> {
        let s: Self = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        s.validate()?;
        Ok(s)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.live_orders_enabled,
            "Real-order activation is not supported"
        );
        ensure!(
            !self.atr_stop_enabled,
            "Selected production strategy has no added ATR stop"
        );
        ensure!(
            self.strategy == "supertrend_macd_vwap"
                && self.instrument == "CRUDEOIL26SEPFUT.MCX"
                && self.interval == "5minute"
                && self.contracts == 1
                && self.supertrend_period == 7
                && self.supertrend_multiplier == 2
                && self.macd_fast == 12
                && self.macd_slow == 26
                && self.macd_signal == 9
                && self.session_vwap,
            "Selection differs from the reviewed five-minute strategy"
        );
        Ok(())
    }
}
pub fn preflight(path: &str) -> Result<()> {
    let _ = Selection::load(path)?;
    println!(
        "{}",
        serde_json::json!({"event":"production_readiness","selection_valid":true,
        "strategy":"supertrend_macd_vwap","interval":"5minute","contracts":1,"atr_stop_enabled":false,
        "live_orders_enabled":false,"ready_for_live_deployment":false,
        "live_node_paper_integrated":true,
        "blockers":["Paper operation is bounded; continuous full-session operation and automatic recovery are not enabled",
        "Real Kite order submission remains disabled; manual review and broker validation remain outstanding"]})
    );
    anyhow::bail!(
        "Real production activation blocked; selected strategy supports bounded LiveNode paper operation"
    )
}
pub fn verify(config: &str, date: &str, input: &str, folder: &str) -> Result<()> {
    let s = Selection::load(config)?;
    let date_value = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
    let data = super::supertrend_input::load(date_value, Some(input))?;
    ensure!(
        data.interval == s.interval && data.instrument_id == s.instrument,
        "Replay input differs from production selection"
    );
    super::supertrend_batch::session(date, input, folder, "confirmed")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_rejects_real_orders_stops_and_ten_minute_changes() {
        let v = include_str!("../../../../config/production-supertrend.json");
        let base: Selection = serde_json::from_str(v).unwrap();
        assert!(base.validate().is_ok());
        for (key, value) in [
            ("live_orders_enabled", serde_json::json!(true)),
            ("atr_stop_enabled", serde_json::json!(true)),
            ("interval", serde_json::json!("10minute")),
            ("contracts", serde_json::json!(2)),
        ] {
            let mut modified: serde_json::Value = serde_json::from_str(v).unwrap();
            modified[key] = value;
            let s: Selection = serde_json::from_value(modified).unwrap();
            assert!(s.validate().is_err());
        }
    }
}

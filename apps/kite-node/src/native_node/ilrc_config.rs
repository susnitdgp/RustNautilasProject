use super::{ilrc_backtest::Params, session_calendar::Calendar};
use anyhow::{Result, ensure};
use chrono::{Datelike, NaiveDate};
use kite_adapter::http::historical::Interval;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationSelection {
    pub enabled: bool,
    pub target_r: f64,
    pub break_even_r: f64,
}

impl ContinuationSelection {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.enabled,
            "selected combined ILRC profile requires continuation enabled"
        );
        ensure!(
            self.target_r == 3.0 && self.break_even_r == 1.0,
            "selected continuation profile must use 3R target and 1R break-even"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub strategy: String,
    pub instrument: String,
    pub symbol: String,
    pub instrument_token: u32,
    pub expected_expiry: NaiveDate,
    pub session_calendar: Calendar,
    pub interval: Interval,
    pub contracts: u32,
    pub live_orders_enabled: bool,
    pub session_open_minute: u32,
    pub entry_cutoff_minute: u32,
    pub ilrc: Params,
    pub continuation: ContinuationSelection,
}

impl Selection {
    pub fn load(path: &str) -> Result<Self> {
        let value: Self = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_mode(false)
    }

    pub fn load_live(path: &str) -> Result<Self> {
        let value: Self = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        value.validate_live()?;
        Ok(value)
    }
    pub fn validate_live(&self) -> Result<()> {
        self.validate_mode(true)
    }

    fn validate_mode(&self, live: bool) -> Result<()> {
        self.session_calendar.validate()?;
        self.ilrc.validate()?;
        self.continuation.validate()?;
        ensure!(
            self.strategy == "institutional_liquidity_reversal_continuation_v1",
            "Only ILRC v1 is supported by this selection"
        );
        ensure!(
            self.interval == Interval::ThreeMinute,
            "ILRC v1 selected profile must use 3-minute bars"
        );
        ensure!(
            self.live_orders_enabled == live,
            "ILRC strategy mode does not match configured live-order gate"
        );
        ensure!(
            self.contracts == 1,
            "ILRC v1 research profile requires one contract"
        );
        ensure!(
            self.symbol
                == format!(
                    "CRUDEOIL{}FUT",
                    self.expected_expiry
                        .format("%y%b")
                        .to_string()
                        .to_uppercase()
                )
                && (2020..=2099).contains(&self.expected_expiry.year()),
            "Configured symbol and expiry disagree"
        );
        ensure!(
            self.instrument == format!("{}.MCX", self.symbol)
                && self.instrument_token > 0
                && kite_adapter::instruments::contract::validate_symbol(&self.symbol).is_ok(),
            "ILRC selection requires a valid MCX CRUDEOIL future"
        );
        ensure!(
            self.expected_expiry >= self.session_calendar.valid_from
                && self.expected_expiry <= self.session_calendar.valid_through,
            "Session calendar must cover expiry"
        );
        ensure!(
            self.session_open_minute == 540 && self.entry_cutoff_minute == 1395,
            "Selected CRUDEOIL ILRC profile must use 09:00-23:15 IST"
        );
        Ok(())
    }
}

pub fn production_check(config: &str, broker: &str) -> Result<()> {
    let s = Selection::load(config)?;
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(broker)?)?;
    let broker_live = raw
        .get("live_orders_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    ensure!(
        !broker_live,
        "ILRC production-shadow deployment requires broker live_orders_enabled=false"
    );
    println!(
        "{}",
        serde_json::json!({
            "event":"ilrc_v1_configuration_check",
            "configuration_valid":true,
            "strategy":s.strategy,
            "interval":s.interval.as_str(),
            "instrument":s.instrument,
            "instrument_token":s.instrument_token,
            "contracts":s.contracts,
            "strategy_live_orders_enabled":s.live_orders_enabled,
            "broker_live_orders_enabled":broker_live,
            "broker_orders_sent":false,
            "execution_path_enabled":false,
            "continuation_enabled":s.continuation.enabled,
            "continuation_target_r":s.continuation.target_r,
            "continuation_break_even_r":s.continuation.break_even_r
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_ilrc_json_is_three_minute_and_live_disabled() {
        let s: Selection =
            serde_json::from_str(include_str!("../../../../config/production-ilrc.json")).unwrap();
        s.validate().unwrap();
        assert_eq!(
            s.strategy,
            "institutional_liquidity_reversal_continuation_v1"
        );
        assert_eq!(s.interval, Interval::ThreeMinute);
        assert_eq!(s.contracts, 1);
        assert!(!s.live_orders_enabled);
        assert_eq!(s.ilrc.atr_len, 14);
        assert_eq!(s.ilrc.swing_len, 20);
        assert_eq!(s.ilrc.retrace_bars, 5);
        assert_eq!(s.ilrc.min_rr, 1.5);
        assert!(s.continuation.enabled);
        assert_eq!(s.continuation.target_r, 3.0);
        assert_eq!(s.continuation.break_even_r, 1.0);
    }
}

//! Validated production selection and offline release verification; no live activation.
use super::session_calendar::Calendar;
use anyhow::{Result, ensure};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    strategy: String,
    pub instrument: String,
    pub symbol: String,
    pub instrument_token: u32,
    pub expected_expiry: NaiveDate,
    pub session_calendar: Calendar,
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
    pub fn resolve(
        &self,
        master: &[u8],
        date: NaiveDate,
    ) -> Result<kite_adapter::preflight::Report> {
        self.validate()?;
        let report = kite_adapter::preflight::run_selected(
            &self.symbol,
            self.instrument_token,
            master,
            date,
        )?;
        ensure!(
            report.instrument_id == self.instrument
                && report.expiry == self.expected_expiry.to_string(),
            "JSON instrument or expected expiry differs from the selected Kite contract"
        );
        Ok(report)
    }
    /// Synthetic metadata is only for offline simulation; retain the configured routing identity.
    pub fn synthetic_instrument(&self) -> Result<nautilus_model::instruments::FuturesContract> {
        self.validate()?;
        let (mut instrument, _) = crate::paper_flow::simulation::live_clock_fixture()?;
        instrument.id = self.instrument.parse()?;
        instrument.raw_symbol = self.symbol.as_str().into();
        Ok(instrument)
    }
    fn validate(&self) -> Result<()> {
        self.session_calendar.validate()?;
        ensure!(
            self.expected_expiry >= self.session_calendar.valid_from
                && self.expected_expiry <= self.session_calendar.valid_through,
            "JSON session calendar must cover the configured contract expiry"
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
            "Configured symbol and expected expiry month disagree"
        );
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
                && kite_adapter::instruments::contract::validate_symbol(&self.symbol).is_ok()
                && self.instrument == format!("{}.MCX", self.symbol)
                && self.instrument_token > 0
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
/// Read-only contract/calendar check; never loads credentials, starts a node or sends orders.
pub fn contract_check(path: &str) -> Result<()> {
    let selection = Selection::load(path)?;
    let now = chrono::Utc::now();
    let date = now
        .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
        .date_naive();
    let session = selection.session_calendar.session(date)?;
    let master = kite_adapter::http::instruments::download()?;
    let report = selection.resolve(&master, date)?;
    let instrument =
        kite_adapter::instruments::contract::build(&report, super::data::now().into())?;
    println!(
        "{}",
        serde_json::json!({
            "event":"selected_contract_check", "instrument":instrument.id.to_string(),
            "instrument_token":report.instrument_token, "expiry":report.expiry,
            "broker_lot_size":report.broker_lot_size, "tick_size":report.tick_size,
            "validation_date_ist":date, "session_today":session.is_some(),
            "calendar_valid_through":selection.session_calendar.valid_through,
            "live_orders_enabled":false, "engine_started":false, "orders_sent":0
        })
    );
    Ok(())
}
pub fn preflight(path: &str) -> Result<()> {
    let _ = Selection::load(path)?;
    println!(
        "{}",
        serde_json::json!({"event":"production_readiness","selection_valid":true,
        "strategy":"supertrend_macd_vwap","interval":"5minute","contracts":1,"atr_stop_enabled":false,
        "live_orders_enabled":false,"ready_for_live_deployment":false,
        "live_node_paper_integrated":true,
        "blockers":["Session paper operation and indicator recovery are implemented; full-session qualification and manual review remain",
        "Protected-market native broker wiring exists; default build/config disable submission and real broker validation remains outstanding"]})
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
    fn selected_metadata_matches_master_and_simulated_routing() {
        let config = include_str!("../../../../config/production-supertrend.json");
        let selection: Selection = serde_json::from_str(config).unwrap();
        let master = format!("instrument_token,tradingsymbol,name,expiry,tick_size,lot_size,instrument_type,segment,exchange\n{}, {},CRUDEOIL,{},1,1,FUT,MCX-FUT,MCX\n",
            selection.instrument_token, selection.symbol, selection.expected_expiry).replace(", ", ",");
        let date = NaiveDate::from_ymd_opt(2026, 9, 22).unwrap();
        let report = selection.resolve(master.as_bytes(), date).unwrap();
        let live =
            kite_adapter::instruments::contract::build(&report, super::super::data::now().into())
                .unwrap();
        let sim = selection.synthetic_instrument().unwrap();
        assert_eq!(live.id, sim.id);
        assert_eq!(live.raw_symbol, sim.raw_symbol);
        assert_eq!(live.id.to_string(), selection.instrument);
        let wrong_expiry = master.replace("2026-10-19", "2026-10-20");
        assert!(selection.resolve(wrong_expiry.as_bytes(), date).is_err());
        let wrong_token = master.replace(&selection.instrument_token.to_string(), "144870151");
        assert!(selection.resolve(wrong_token.as_bytes(), date).is_err());
        assert!(
            selection
                .resolve(
                    master.as_bytes(),
                    NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()
                )
                .is_err()
        );
    }
    #[test]
    fn missing_calendar_wrong_expiry_month_and_short_coverage_fail_closed() {
        let base: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../config/production-supertrend.json"
        ))
        .unwrap();
        let mut value = base.clone();
        value.as_object_mut().unwrap().remove("session_calendar");
        assert!(serde_json::from_value::<Selection>(value).is_err());
        let mut value = base.clone();
        value["expected_expiry"] = serde_json::json!("2026-09-21");
        assert!(
            serde_json::from_value::<Selection>(value)
                .unwrap()
                .validate()
                .is_err()
        );
        let mut value = base;
        value["session_calendar"]["valid_through"] = serde_json::json!("2026-10-18");
        assert!(
            serde_json::from_value::<Selection>(value)
                .unwrap()
                .validate()
                .is_err()
        );
    }
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

    #[test]
    fn selection_accepts_explicit_rollover_identity_and_rejects_mismatch() {
        let v = include_str!("../../../../config/production-supertrend.json");
        let mut rolled: serde_json::Value = serde_json::from_str(v).unwrap();
        rolled["instrument"] = serde_json::json!("CRUDEOIL26OCTFUT.MCX");
        rolled["symbol"] = serde_json::json!("CRUDEOIL26OCTFUT");
        rolled["instrument_token"] = serde_json::json!(155_000_001);
        let selection: Selection = serde_json::from_value(rolled.clone()).unwrap();
        assert!(selection.validate().is_ok());

        rolled["instrument"] = serde_json::json!("CRUDEOIL26SEPFUT.MCX");
        let mismatched: Selection = serde_json::from_value(rolled.clone()).unwrap();
        assert!(mismatched.validate().is_err());

        rolled["instrument"] = serde_json::json!("CRUDEOIL26OCTFUT.MCX");
        rolled["instrument_token"] = serde_json::json!(0);
        let zero_token: Selection = serde_json::from_value(rolled).unwrap();
        assert!(zero_token.validate().is_err());
    }
}

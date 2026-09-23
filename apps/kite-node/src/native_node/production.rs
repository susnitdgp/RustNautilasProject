//! Validated strategy selection, production gates and offline release verification.
use super::session_calendar::Calendar;
use anyhow::{Result, ensure};
use chrono::{Datelike, NaiveDate};
use kite_adapter::http::historical::Interval;
use serde::Deserialize;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub strategy: String,
    pub instrument: String,
    pub symbol: String,
    pub instrument_token: u32,
    pub expected_expiry: NaiveDate,
    pub session_calendar: Calendar,
    pub interval: Interval,
    contracts: u32,
    supertrend_period: Option<u32>,
    supertrend_multiplier: Option<u32>,
    macd_fast: Option<u32>,
    macd_slow: Option<u32>,
    macd_signal: Option<u32>,
    session_vwap: Option<bool>,
    atr_stop_enabled: bool,
    live_orders_enabled: bool,
    pub pivot_point: Option<super::pivot_point::Settings>,
    pub trend_ribbon: Option<super::trend_ribbon::Settings>,
}
impl Selection {
    pub fn load(path: &str) -> Result<Self> {
        let s: Self = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        s.validate()?;
        Ok(s)
    }
    pub const fn interval_name(&self) -> &'static str {
        self.interval.as_str()
    }
    pub const fn interval_minutes(&self) -> u64 {
        self.interval.minutes()
    }
    pub const fn bar_ns(&self) -> u64 {
        self.interval.nanoseconds()
    }
    pub fn bar_type(
        &self,
        instrument: &nautilus_model::identifiers::InstrumentId,
    ) -> Result<nautilus_model::data::BarType> {
        Ok(format!(
            "{}-{}-MINUTE-LAST-EXTERNAL",
            instrument,
            self.interval_minutes()
        )
        .parse()?)
    }
    pub fn session_bounds(&self, date: NaiveDate) -> Result<(u64, u64)> {
        if let Some(ribbon) = &self.trend_ribbon {
            return ribbon
                .session
                .window(date, &self.session_calendar)?
                .ok_or_else(|| anyhow::anyhow!("No Trend Ribbon trading session for {date}"));
        }
        if let Some(pivot) = &self.pivot_point {
            pivot
                .session
                .window(date, &self.session_calendar)?
                .ok_or_else(|| anyhow::anyhow!("No Pivot Point trading session for {date}"))
        } else {
            self.session_calendar.bounds(date)
        }
    }
    pub fn session_duration(&self, now: u64) -> Result<u64> {
        if self.pivot_point.is_none() && self.trend_ribbon.is_none() {
            return super::supertrend_session::duration(now, &self.session_calendar);
        }
        let (start, end) = self.session_bounds(super::pivot_session::date(now))?;
        ensure!(
            now >= start && now + 5_000_000_000 < end,
            "Start Pivot Point during its configured session and before square-off"
        );
        Ok((end - now).div_ceil(1_000_000_000))
    }
    /// Effective live cutoff retains the application's existing MIS exit buffer.
    pub fn execution_bounds(&self, date: NaiveDate, real: bool) -> Result<(u64, u64)> {
        let (start, mut end) = self.session_bounds(date)?;
        if real && (self.pivot_point.is_some() || self.trend_ribbon.is_some()) {
            let (_, market_close) = self.session_calendar.bounds(date)?;
            let cutoff = market_close
                .checked_sub(super::supertrend_session::EXIT_BUFFER_SECONDS * 1_000_000_000)
                .ok_or_else(|| anyhow::anyhow!("Invalid production session cutoff"))?;
            end = end.min(cutoff);
            ensure!(
                start < end,
                "Pivot production session ends before its configured start"
            );
        }
        Ok((start, end))
    }
    pub fn production_duration(&self, now: u64) -> Result<u64> {
        if self.pivot_point.is_none() && self.trend_ribbon.is_none() {
            return self.session_duration(now);
        }
        let (start, end) = self.execution_bounds(super::pivot_session::date(now), true)?;
        ensure!(
            now >= start && now + 15_000_000_000 < end,
            "Start Pivot production during its session and before the MIS application exit window"
        );
        Ok((end - now).div_ceil(1_000_000_000))
    }
    pub fn broker_settings(
        &self,
        path: &str,
    ) -> Result<kite_adapter::execution::native_client::production::Settings> {
        if self.pivot_point.is_some() || self.trend_ribbon.is_some() {
            ensure!(
                self.live_orders_enabled,
                "Pivot production requires live_orders_enabled=true in the strategy JSON; paper selection cannot start real orders"
            );
        }
        let mut settings: kite_adapter::execution::native_client::production::Settings =
            serde_json::from_str(&std::fs::read_to_string(path)?)?;
        // The strategy selection is the routing source of truth for all live modes.
        settings.instrument_token = self.instrument_token;
        settings.validate()?;
        Ok(settings)
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
            !self.atr_stop_enabled,
            "Selected production strategy has no added ATR stop"
        );
        ensure!(
            kite_adapter::instruments::contract::validate_symbol(&self.symbol).is_ok()
                && self.instrument == format!("{}.MCX", self.symbol)
                && self.instrument_token > 0
                && self.contracts == 1,
            "Selection requires one configured MCX crude oil contract"
        );
        match (
            self.strategy.as_str(),
            &self.pivot_point,
            &self.trend_ribbon,
        ) {
            ("supertrend_macd_vwap", None, None) => ensure!(
                !self.live_orders_enabled
                    && self.interval == Interval::FiveMinute
                    && self.supertrend_period == Some(7)
                    && self.supertrend_multiplier == Some(2)
                    && self.macd_fast == Some(12)
                    && self.macd_slow == Some(26)
                    && self.macd_signal == Some(9)
                    && self.session_vwap == Some(true),
                "Selection differs from the reviewed five-minute strategy"
            ),
            ("pivot_point_supertrend", Some(pivot), None) => {
                pivot.validate()?;
                ensure!(
                    self.interval == Interval::FiveMinute
                        && self.supertrend_period.is_none()
                        && self.supertrend_multiplier.is_none()
                        && self.macd_fast.is_none()
                        && self.macd_slow.is_none()
                        && self.macd_signal.is_none()
                        && self.session_vwap.is_none(),
                    "Pivot Point strategy does not use ordinary Supertrend or MACD/VWAP settings"
                );
            }
            ("trend_ribbon_boswaves", None, Some(ribbon)) => {
                ribbon.validate()?;
                ensure!(
                    self.supertrend_period.is_none()
                        && self.supertrend_multiplier.is_none()
                        && self.macd_fast.is_none()
                        && self.macd_slow.is_none()
                        && self.macd_signal.is_none()
                        && self.session_vwap.is_none(),
                    "Trend Ribbon does not use Supertrend or MACD/VWAP settings"
                );
            }
            _ => anyhow::bail!("Unsupported strategy or mismatched strategy configuration"),
        }
        for date in self.session_calendar.range(
            self.session_calendar.valid_from,
            self.session_calendar.valid_through,
        )? {
            let (start, end) = self.session_bounds(date)?;
            ensure!(
                start.is_multiple_of(self.bar_ns()) && end.is_multiple_of(self.bar_ns()),
                "Configured strategy session must align with the selected candle interval"
            );
        }
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
            "interval":selection.interval_name(),
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
    let selection = Selection::load(path)?;
    println!(
        "{}",
        serde_json::json!({"event":"production_readiness","selection_valid":true,
        "strategy":selection.strategy,"interval":selection.interval_name(),"contracts":1,"atr_stop_enabled":false,
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
    ensure!(
        s.pivot_point.is_none(),
        "Legacy production replay is only for Supertrend + MACD/VWAP; use the Pivot Point simulation command"
    );
    let date_value = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
    let data = super::supertrend_input::load(date_value, Some(input))?;
    ensure!(
        data.interval == s.interval_name() && data.instrument_id == s.instrument,
        "Replay input differs from production selection"
    );
    super::supertrend_batch::session(date, input, folder, "confirmed")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pivot_selection_validates_settings_and_uses_its_own_session_deadline() {
        let raw = include_str!("../../../../config/backup/pivot-point-supertrend.json");
        let selection: Selection = serde_json::from_str(raw).unwrap();
        selection.validate().unwrap();
        assert_eq!(selection.strategy, "pivot_point_supertrend");
        let ns = |s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .timestamp_nanos_opt()
                .unwrap() as u64
        };
        assert_eq!(
            selection
                .session_duration(ns("2026-09-22T09:00:00+05:30"))
                .unwrap(),
            14 * 3600 + 15 * 60
        );
        assert_eq!(
            selection
                .session_duration(ns("2026-09-22T23:14:50.5+05:30"))
                .unwrap(),
            10
        );
        assert!(
            selection
                .session_duration(ns("2026-09-22T23:15:00+05:30"))
                .is_err()
        );
        assert!(
            selection
                .session_duration(ns("2026-10-02T09:00:00+05:30"))
                .is_err()
        );
        for (key, value) in [
            ("strategy", serde_json::json!("supertrend_macd_vwap")),
            ("macd_fast", serde_json::json!(12)),
        ] {
            let mut invalid: serde_json::Value = serde_json::from_str(raw).unwrap();
            invalid[key] = value;
            assert!(
                serde_json::from_value::<Selection>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }
    #[test]
    fn selected_metadata_matches_master_and_simulated_routing() {
        let config = include_str!("../../../../config/backup/production-supertrend.json");
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
            "../../../../config/backup/production-supertrend.json"
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
        let v = include_str!("../../../../config/backup/production-supertrend.json");
        let base: Selection = serde_json::from_str(v).unwrap();
        assert!(base.validate().is_ok());
        for (key, value) in [
            ("live_orders_enabled", serde_json::json!(true)),
            ("atr_stop_enabled", serde_json::json!(true)),
            ("contracts", serde_json::json!(2)),
        ] {
            let mut modified: serde_json::Value = serde_json::from_str(v).unwrap();
            modified[key] = value;
            let s: Selection = serde_json::from_value(modified).unwrap();
            assert!(s.validate().is_err());
        }
        let mut unsupported: serde_json::Value = serde_json::from_str(v).unwrap();
        unsupported["interval"] = serde_json::json!("10minute");
        assert!(serde_json::from_value::<Selection>(unsupported).is_err());
        let mut three_minute: serde_json::Value = serde_json::from_str(v).unwrap();
        three_minute["interval"] = serde_json::json!("3minute");
        assert!(
            serde_json::from_value::<Selection>(three_minute)
                .unwrap()
                .validate()
                .is_err()
        );
    }

    #[test]
    fn selection_accepts_explicit_rollover_identity_and_rejects_mismatch() {
        let v = include_str!("../../../../config/backup/production-supertrend.json");
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
    #[test]
    fn trend_ribbon_interval_is_selected_from_json() {
        let mut value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../config/production-trend-ribbon.json"
        ))
        .unwrap();
        value["interval"] = serde_json::json!("3minute");
        let selection: Selection = serde_json::from_value(value).unwrap();
        selection.validate().unwrap();
        assert_eq!(selection.interval, Interval::ThreeMinute);
        assert_eq!(selection.interval_name(), "3minute");
        assert_eq!(selection.interval_minutes(), 3);
        assert_eq!(selection.bar_ns(), 180_000_000_000);
        let instrument: nautilus_model::identifiers::InstrumentId =
            "CRUDEOIL26OCTFUT.MCX".parse().unwrap();
        assert_eq!(
            selection.bar_type(&instrument).unwrap().to_string(),
            "CRUDEOIL26OCTFUT.MCX-3-MINUTE-LAST-EXTERNAL"
        );
        let mut misaligned: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../config/production-trend-ribbon.json"
        ))
        .unwrap();
        misaligned["trend_ribbon"]["session"]["end"] = serde_json::json!("23:14:00");
        let misaligned: Selection = serde_json::from_value(misaligned).unwrap();
        assert!(misaligned.validate().is_err());
    }
}

#[cfg(test)]
#[path = "pivot_production_tests.rs"]
mod pivot_production_tests;

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
    atr_stop_enabled: bool,
    live_orders_enabled: bool,
    pub trend_ribbon: super::trend_ribbon::Settings,
}

impl Selection {
    pub fn load(path: &str) -> Result<Self> {
        let selection: Self = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        selection.validate()?;
        Ok(selection)
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
        self.trend_ribbon
            .session
            .window(date, &self.session_calendar)?
            .ok_or_else(|| anyhow::anyhow!("No Trend Ribbon trading session for {date}"))
    }

    /// Effective live cutoff retains the application's MIS exit buffer.
    pub fn execution_bounds(&self, date: NaiveDate, real: bool) -> Result<(u64, u64)> {
        let (start, mut end) = self.session_bounds(date)?;
        if real {
            let (_, market_close) = self.session_calendar.bounds(date)?;
            let cutoff = market_close
                .checked_sub(super::execution_session::EXIT_BUFFER_SECONDS * 1_000_000_000)
                .ok_or_else(|| anyhow::anyhow!("Invalid production session cutoff"))?;
            end = end.min(cutoff);
            ensure!(
                start < end,
                "Production session ends before its configured start"
            );
        }
        Ok((start, end))
    }

    pub fn production_duration(&self, now: u64) -> Result<u64> {
        let (start, end) = self.execution_bounds(super::strategy_session::date(now), true)?;
        ensure!(
            now >= start && now + 15_000_000_000 < end,
            "Start Trend Ribbon production during its session and before the exit window"
        );
        Ok((end - now).div_ceil(1_000_000_000))
    }

    pub fn broker_settings(
        &self,
        path: &str,
    ) -> Result<kite_adapter::execution::native_client::production::Settings> {
        ensure!(
            self.live_orders_enabled,
            "Trend Ribbon production requires live_orders_enabled=true in the strategy JSON"
        );
        let mut settings: kite_adapter::execution::native_client::production::Settings =
            serde_json::from_str(&std::fs::read_to_string(path)?)?;
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

    /// Synthetic metadata is only for offline simulation.
    pub fn synthetic_instrument(&self) -> Result<nautilus_model::instruments::FuturesContract> {
        self.validate()?;
        let (mut instrument, _) = super::synthetic::live_clock_fixture()?;
        instrument.id = self.instrument.parse()?;
        instrument.raw_symbol = self.symbol.as_str().into();
        Ok(instrument)
    }

    fn validate(&self) -> Result<()> {
        self.session_calendar.validate()?;
        ensure!(
            self.strategy == "trend_ribbon_boswaves",
            "Only trend_ribbon_boswaves is supported by the active selection"
        );
        ensure!(
            self.expected_expiry >= self.session_calendar.valid_from
                && self.expected_expiry <= self.session_calendar.valid_through,
            "Session calendar must cover the configured contract expiry"
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
        ensure!(!self.atr_stop_enabled, "Trend Ribbon has no added ATR stop");
        ensure!(
            kite_adapter::instruments::contract::validate_symbol(&self.symbol).is_ok()
                && self.instrument == format!("{}.MCX", self.symbol)
                && self.instrument_token > 0
                && self.contracts == 1,
            "Selection requires one configured MCX crude oil contract"
        );
        self.trend_ribbon.validate()?;
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

/// Read-only contract/calendar check; never loads credentials or sends orders.
pub fn contract_check(path: &str) -> Result<()> {
    let selection = Selection::load(path)?;
    let now = chrono::Utc::now();
    let date = now
        .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"))
        .date_naive();
    let session = selection.session_calendar.session(date)?;
    let master = kite_adapter::http::instruments::download()?;
    let report = selection.resolve(&master, date)?;
    let instrument =
        kite_adapter::instruments::contract::build(&report, super::data::now().into())?;
    println!(
        "{}",
        serde_json::json!({
            "event":"selected_contract_check",
            "instrument":instrument.id.to_string(),
            "interval":selection.interval_name(),
            "instrument_token":report.instrument_token,
            "expiry":report.expiry,
            "broker_lot_size":report.broker_lot_size,
            "tick_size":report.tick_size,
            "validation_date_ist":date,
            "session_today":session.is_some(),
            "calendar_valid_through":selection.session_calendar.valid_through,
            "live_orders_enabled":false,
            "engine_started":false,
            "orders_sent":0
        })
    );
    Ok(())
}

pub fn preflight(path: &str) -> Result<()> {
    let selection = Selection::load(path)?;
    println!(
        "{}",
        serde_json::json!({
            "event":"production_readiness",
            "selection_valid":true,
            "strategy":selection.strategy,
            "interval":selection.interval_name(),
            "contracts":1,
            "atr_stop_enabled":false,
            "live_orders_enabled":false,
            "ready_for_live_deployment":false
        })
    );
    anyhow::bail!("Production activation remains explicitly gated")
}

/// Offline production configuration check. No account access or order submission.
pub fn production_check(config: &str, broker: &str) -> Result<()> {
    let selection = Selection::load(config)?;
    let settings = selection.broker_settings(broker)?;
    let now = super::data::now();
    let date = super::strategy_session::date(now);
    let window = selection.execution_bounds(date, true).ok();
    let ist = |ns: u64| {
        chrono::DateTime::from_timestamp_nanos(ns as i64)
            .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"))
            .to_rfc3339()
    };
    println!(
        "{}",
        serde_json::json!({
            "event":"trend_ribbon_production_configuration_check",
            "configuration_valid":true,
            "strategy":selection.strategy,
            "interval":selection.interval_name(),
            "instrument":selection.instrument,
            "instrument_token":settings.instrument_token,
            "product":settings.product,
            "market_protection":settings.market_protection,
            "configured_live_orders_enabled":settings.live_orders_enabled,
            "session_start_ist":window.map(|(start,_)|ist(start)),
            "square_off_ist":window.map(|(_,end)|ist(end)),
            "can_start_now_by_calendar":selection.production_duration(now).is_ok(),
            "market_exit_buffer_seconds":super::execution_session::EXIT_BUFFER_SECONDS,
            "engine_started":false,
            "account_checked":false,
            "instrument_master_checked":false,
            "broker_orders_sent":false
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> Selection {
        serde_json::from_str(include_str!(
            "../../../../config/production-trend-ribbon.json"
        ))
        .unwrap()
    }

    #[test]
    fn v210_candidate_is_valid_and_live_orders_are_disabled() {
        let selection = selection();
        selection.validate().unwrap();
        assert_eq!(selection.interval, Interval::FiveMinute);
        assert!(!selection.live_orders_enabled);
        assert!(selection.trend_ribbon.session.reset_daily);
        assert!(selection.trend_ribbon.realtime.enabled);
        assert!(selection.trend_ribbon.realtime.fast_reversal_enabled);
        assert!(selection.trend_ribbon.realtime.wt_exit_enabled);
        assert_eq!(selection.trend_ribbon.realtime.pre_close_seconds, 3);
        assert_eq!(selection.trend_ribbon.realtime.fast_hold_seconds, 2);
        assert_eq!(selection.trend_ribbon.realtime.wt_pullback_points, 5.0);
    }

    #[test]
    fn selected_metadata_matches_master_and_simulated_routing() {
        let selection = selection();
        let master = format!(
            "instrument_token,tradingsymbol,name,expiry,tick_size,lot_size,instrument_type,segment,exchange\n{},{},CRUDEOIL,{},1,1,FUT,MCX-FUT,MCX\n",
            selection.instrument_token, selection.symbol, selection.expected_expiry
        );
        let date = NaiveDate::from_ymd_opt(2026, 9, 22).unwrap();
        let report = selection.resolve(master.as_bytes(), date).unwrap();
        let live =
            kite_adapter::instruments::contract::build(&report, super::super::data::now().into())
                .unwrap();
        let sim = selection.synthetic_instrument().unwrap();
        assert_eq!(live.id, sim.id);
        assert_eq!(live.raw_symbol, sim.raw_symbol);
        assert_eq!(live.id.to_string(), selection.instrument);
    }

    #[test]
    fn invalid_identity_calendar_or_risk_shape_fails_closed() {
        let base: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../config/production-trend-ribbon.json"
        ))
        .unwrap();

        for mutate in [
            ("expected_expiry", serde_json::json!("2026-09-21")),
            ("atr_stop_enabled", serde_json::json!(true)),
            ("contracts", serde_json::json!(2)),
            ("strategy", serde_json::json!("pivot_point_supertrend")),
        ] {
            let mut value = base.clone();
            value[mutate.0] = mutate.1;
            let parsed = serde_json::from_value::<Selection>(value);
            assert!(parsed.is_err() || parsed.unwrap().validate().is_err());
        }

        let mut short_calendar = base.clone();
        short_calendar["session_calendar"]["valid_through"] = serde_json::json!("2026-10-18");
        assert!(
            serde_json::from_value::<Selection>(short_calendar)
                .unwrap()
                .validate()
                .is_err()
        );
    }

    #[test]
    fn interval_is_selected_from_json_and_must_align_with_session() {
        let mut value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../config/production-trend-ribbon.json"
        ))
        .unwrap();
        value["interval"] = serde_json::json!("3minute");
        let selection: Selection = serde_json::from_value(value).unwrap();
        selection.validate().unwrap();
        assert_eq!(selection.interval, Interval::ThreeMinute);
        assert_eq!(selection.bar_ns(), 180_000_000_000);

        let mut misaligned: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../config/production-trend-ribbon.json"
        ))
        .unwrap();
        misaligned["trend_ribbon"]["session"]["end"] = serde_json::json!("23:14:00");
        assert!(
            serde_json::from_value::<Selection>(misaligned)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}

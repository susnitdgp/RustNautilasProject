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
    pub contracts: u32,
    pub live_orders_enabled: bool,
    pub smbc: super::smbc_strategy::Settings,
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
        self.smbc
            .session
            .window(date, &self.session_calendar)?
            .ok_or_else(|| anyhow::anyhow!("No SMBC trading session for {date}"))
    }
    pub fn execution_bounds(&self, date: NaiveDate, real: bool) -> Result<(u64, u64)> {
        let (start, _) = self.session_bounds(date)?;
        let (_, market_close) = self.session_calendar.bounds(date)?;
        let end = if real {
            market_close
                .checked_sub(super::execution_session::EXIT_BUFFER_SECONDS * 1_000_000_000)
                .ok_or_else(|| anyhow::anyhow!("Invalid production session cutoff"))?
        } else {
            market_close
        };
        ensure!(start < end, "Runtime session ends before configured start");
        Ok((start, end))
    }
    pub fn production_duration(&self, now: u64) -> Result<u64> {
        let date = super::strategy_session::date(now);
        let (entry_start, entry_end) = self.session_bounds(date)?;
        let (_, runtime_end) = self.execution_bounds(date, true)?;
        ensure!(
            now >= entry_start && now + 15_000_000_000 < entry_end,
            "Start SMBC production during its 09:00-23:15 entry session"
        );
        Ok((runtime_end - now).div_ceil(1_000_000_000))
    }
    pub fn broker_settings(
        &self,
        path: &str,
    ) -> Result<kite_adapter::execution::native_client::production::Settings> {
        ensure!(
            self.live_orders_enabled,
            "SMBC production requires live_orders_enabled=true in strategy JSON"
        );
        let mut s: kite_adapter::execution::native_client::production::Settings =
            serde_json::from_str(&std::fs::read_to_string(path)?)?;
        s.instrument_token = self.instrument_token;
        s.validate()?;
        Ok(s)
    }
    pub fn resolve(
        &self,
        master: &[u8],
        date: NaiveDate,
    ) -> Result<kite_adapter::preflight::Report> {
        self.validate()?;
        let r = kite_adapter::preflight::run_selected(
            &self.symbol,
            self.instrument_token,
            master,
            date,
        )?;
        ensure!(
            r.instrument_id == self.instrument && r.expiry == self.expected_expiry.to_string(),
            "JSON instrument or expiry differs from selected Kite contract"
        );
        Ok(r)
    }
    pub fn synthetic_instrument(&self) -> Result<nautilus_model::instruments::FuturesContract> {
        self.validate()?;
        let (mut i, _) = super::synthetic::live_clock_fixture()?;
        i.id = self.instrument.parse()?;
        i.raw_symbol = self.symbol.as_str().into();
        Ok(i)
    }
    fn validate(&self) -> Result<()> {
        self.session_calendar.validate()?;
        ensure!(
            self.strategy == "smart_money_breakout_channels_v17",
            "Only smart_money_breakout_channels_v17 is supported"
        );
        ensure!(
            self.expected_expiry >= self.session_calendar.valid_from
                && self.expected_expiry <= self.session_calendar.valid_through,
            "Session calendar must cover expiry"
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
            kite_adapter::instruments::contract::validate_symbol(&self.symbol).is_ok()
                && self.instrument == format!("{}.MCX", self.symbol)
                && self.instrument_token > 0
                && self.contracts == 1,
            "Selection requires one MCX crude contract"
        );
        self.smbc.validate()?;
        for date in self.session_calendar.range(
            self.session_calendar.valid_from,
            self.session_calendar.valid_through,
        )? {
            let (start, end) = self.session_bounds(date)?;
            ensure!(
                start.is_multiple_of(self.bar_ns()) && end.is_multiple_of(self.bar_ns()),
                "Strategy session must align with candle interval"
            );
        }
        Ok(())
    }
}

pub fn contract_check(path: &str) -> Result<()> {
    let s = Selection::load(path)?;
    let now = chrono::Utc::now();
    let date = now
        .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"))
        .date_naive();
    let session = s.session_calendar.session(date)?;
    let master = kite_adapter::http::instruments::download()?;
    let report = s.resolve(&master, date)?;
    let instrument =
        kite_adapter::instruments::contract::build(&report, super::data::now().into())?;
    println!(
        "{}",
        serde_json::json!({"event":"selected_contract_check","instrument":instrument.id.to_string(),"interval":s.interval_name(),
        "instrument_token":report.instrument_token,"expiry":report.expiry,"broker_lot_size":report.broker_lot_size,"tick_size":report.tick_size,
        "validation_date_ist":date,"session_today":session.is_some(),"calendar_valid_through":s.session_calendar.valid_through,
        "live_orders_enabled":false,"engine_started":false,"orders_sent":0})
    );
    Ok(())
}
pub fn preflight(path: &str) -> Result<()> {
    let s = Selection::load(path)?;
    println!(
        "{}",
        serde_json::json!({"event":"production_readiness","selection_valid":true,"strategy":s.strategy,
        "interval":s.interval_name(),"contracts":s.contracts,"live_orders_enabled":false,"ready_for_live_deployment":false})
    );
    anyhow::bail!("Production activation remains explicitly gated")
}
pub fn production_check(config: &str, broker: &str) -> Result<()> {
    let s = Selection::load(config)?;
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(broker)?)?;
    let configured_live = raw
        .get("live_orders_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!(
        "{}",
        serde_json::json!({"event":"smbc_production_configuration_check","configuration_valid":true,"strategy":s.strategy,
        "interval":s.interval_name(),"instrument":s.instrument,"instrument_token":s.instrument_token,"strategy_live_orders_enabled":s.live_orders_enabled,
        "broker_live_orders_enabled":configured_live,"engine_started":false,"account_checked":false,"broker_orders_sent":false})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn production_json_is_valid_and_live_disabled() {
        let s: Selection =
            serde_json::from_str(include_str!("../../../../config/production-smbc.json")).unwrap();
        s.validate().unwrap();
        assert_eq!(s.strategy, "smart_money_breakout_channels_v17");
        assert!(!s.live_orders_enabled);
        assert_eq!(s.interval, Interval::OneMinute);
        assert_eq!(s.smbc.normalization_length, 100);
        assert_eq!(s.smbc.box_detection_length, 10);
        assert_eq!(s.smbc.min_stop_points, 8.0);
        assert_eq!(s.smbc.max_stop_points, 18.0);
        assert_eq!(s.smbc.entry_cooldown_bars, 5);
    }
}

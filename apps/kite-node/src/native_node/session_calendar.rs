//! Explicit JSON calendar for live sessions and candle recovery, in IST.
use anyhow::{Result, ensure};
use chrono::{Datelike, FixedOffset, NaiveDate, NaiveTime, TimeZone, Timelike, Weekday};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hours {
    open: NaiveTime,
    close: NaiveTime,
}
impl Hours {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.open < self.close
                && (self.close - self.open).num_seconds()
                    > super::execution_session::EXIT_BUFFER_SECONDS as i64
                && [self.open, self.close]
                    .iter()
                    .all(|t| t.second() == 0 && t.nanosecond() == 0 && t.minute() % 5 == 0),
            "Session hours must be ordered, five-minute aligned, and leave time for shutdown"
        );
        Ok(())
    }
    fn bounds(&self, date: NaiveDate) -> Result<(u64, u64)> {
        let zone = FixedOffset::east_opt(19800).expect("IST");
        let stamp = |time| -> Result<u64> {
            let value = zone
                .from_local_datetime(&date.and_time(time))
                .single()
                .ok_or_else(|| anyhow::anyhow!("Invalid session timestamp"))?;
            Ok(u64::try_from(value.timestamp_nanos_opt().ok_or_else(
                || anyhow::anyhow!("Session timestamp overflow"),
            )?)?)
        };
        Ok((stamp(self.open)?, stamp(self.close)?))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calendar {
    timezone: String,
    pub valid_from: NaiveDate,
    pub valid_through: NaiveDate,
    regular: Hours,
    overrides: BTreeMap<NaiveDate, Option<Hours>>,
}
impl Calendar {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.timezone == "Asia/Kolkata",
            "Session calendar must use Asia/Kolkata"
        );
        ensure!(
            self.valid_from <= self.valid_through
                && (self.valid_through - self.valid_from).num_days() <= 366,
            "Session calendar must cover an explicit range of at most 367 days"
        );
        self.regular.validate()?;
        for (date, hours) in &self.overrides {
            self.check_date(*date)?;
            if let Some(hours) = hours {
                ensure!(
                    !matches!(date.weekday(), Weekday::Sat | Weekday::Sun),
                    "Weekend special sessions are not supported"
                );
                hours.validate()?;
            }
        }
        Ok(())
    }
    fn check_date(&self, date: NaiveDate) -> Result<()> {
        ensure!(
            date >= self.valid_from && date <= self.valid_through,
            "Session date {date} is outside JSON calendar coverage {} through {}; update the reviewed calendar",
            self.valid_from,
            self.valid_through
        );
        Ok(())
    }
    /// None is an explicitly closed holiday or weekend; unknown dates are errors.
    pub fn session(&self, date: NaiveDate) -> Result<Option<(u64, u64)>> {
        self.check_date(date)?;
        if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
            return Ok(None);
        }
        match self.overrides.get(&date) {
            Some(Some(hours)) => Ok(Some(hours.bounds(date)?)),
            Some(None) => Ok(None),
            None => Ok(Some(self.regular.bounds(date)?)),
        }
    }
    pub fn bounds(&self, date: NaiveDate) -> Result<(u64, u64)> {
        self.session(date)?
            .ok_or_else(|| anyhow::anyhow!("No trading session configured for {date}"))
    }
    pub fn range(&self, first: NaiveDate, last: NaiveDate) -> Result<Vec<NaiveDate>> {
        ensure!(first <= last, "Calendar range is reversed");
        self.check_date(first)?;
        self.check_date(last)?;
        let mut days = Vec::new();
        let mut date = first;
        loop {
            if self.session(date)?.is_some() {
                days.push(date);
            }
            if date == last {
                break;
            }
            date = date
                .succ_opt()
                .ok_or_else(|| anyhow::anyhow!("Calendar date overflow"))?;
        }
        Ok(days)
    }
}

#[cfg(test)]
pub fn fixture() -> Calendar {
    serde_json::from_value(serde_json::json!({
        "timezone":"Asia/Kolkata", "valid_from":"2026-08-17", "valid_through":"2026-10-19",
        "regular":{"open":"09:00:00","close":"23:30:00"},
        "overrides":{"2026-09-14":{"open":"17:00:00","close":"23:30:00"},"2026-10-02":null}
    }))
    .unwrap()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn day(value: &str) -> NaiveDate {
        value.parse().unwrap()
    }
    #[test]
    fn october_rollover_holidays_weekends_and_unknown_dates() {
        let calendar = fixture();
        calendar.validate().unwrap();
        let (a, b) = calendar.bounds(day("2026-09-22")).unwrap();
        assert_eq!((b - a) / 1_000_000_000, 14 * 3600 + 1800);
        let (a, b) = calendar.bounds(day("2026-09-14")).unwrap();
        assert_eq!((b - a) / 1_000_000_000, 6 * 3600 + 1800);
        assert!(calendar.bounds(day("2026-10-02")).is_err());
        assert!(calendar.bounds(day("2026-10-03")).is_err());
        assert!(calendar.bounds(day("2026-10-19")).is_ok());
        assert!(calendar.session(day("2026-10-20")).is_err());
        assert_eq!(
            calendar
                .range(day("2026-10-01"), day("2026-10-05"))
                .unwrap(),
            vec![day("2026-10-01"), day("2026-10-05")]
        );
    }
    #[test]
    fn invalid_calendars_fail_closed() {
        let mut c = fixture();
        c.timezone = "UTC".into();
        assert!(c.validate().is_err());
        let mut c = fixture();
        c.regular.close = c.regular.open;
        assert!(c.validate().is_err());
        let mut c = fixture();
        c.regular.open = "09:01:00".parse().unwrap();
        assert!(c.validate().is_err());
        let mut c = fixture();
        c.overrides.insert(day("2026-10-20"), None);
        assert!(c.validate().is_err());
    }
}

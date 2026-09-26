//! Explicit same-day IST session policy for the Pivot Point strategy.
use super::session_calendar::Calendar;
use anyhow::{Result, ensure};
use chrono::{Datelike, FixedOffset, NaiveDate, NaiveTime, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

pub const BAR_NS: u64 = 300_000_000_000;
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub start: NaiveTime,
    pub end: NaiveTime,
    pub days: String,
    pub reset_daily: bool,
}
impl Session {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.start < self.end,
            "Pivot session must start and end on the same IST day"
        );
        ensure!(
            [self.start, self.end]
                .iter()
                .all(|t| t.second() == 0 && t.nanosecond() == 0 && t.minute() % 5 == 0),
            "Pivot session must align with five-minute candles"
        );
        let days: std::collections::BTreeSet<_> = self.days.bytes().collect();
        ensure!(
            !days.is_empty()
                && days.len() == self.days.len()
                && days.iter().all(|d| (b'1'..=b'7').contains(d)),
            "Invalid Pivot session days (1=Sunday .. 7=Saturday)"
        );
        Ok(())
    }
    pub fn window(&self, date: NaiveDate, calendar: &Calendar) -> Result<Option<(u64, u64)>> {
        let weekday = b'1' + date.weekday().num_days_from_sunday() as u8;
        if !self.days.bytes().any(|d| d == weekday) {
            return Ok(None);
        }
        let Some((market_start, market_end)) = calendar.session(date)? else {
            return Ok(None);
        };
        let zone = FixedOffset::east_opt(19_800).expect("IST");
        let stamp = |t| -> Result<u64> {
            let utc = zone
                .from_local_datetime(&date.and_time(t))
                .single()
                .ok_or_else(|| anyhow::anyhow!("Invalid Pivot session time"))?;
            Ok(u64::try_from(utc.timestamp_nanos_opt().ok_or_else(
                || anyhow::anyhow!("Pivot session timestamp overflow"),
            )?)?)
        };
        let start = market_start.max(stamp(self.start)?);
        let end = market_end.min(stamp(self.end)?);
        Ok((start < end).then_some((start, end)))
    }
    pub fn contains(&self, ns: u64, calendar: &Calendar) -> Result<bool> {
        Ok(self
            .window(date(ns), calendar)?
            .is_some_and(|(start, end)| ns >= start && ns < end))
    }
}
pub fn date(ns: u64) -> NaiveDate {
    chrono::DateTime::from_timestamp_nanos(ns as i64)
        .with_timezone(&FixedOffset::east_opt(19_800).expect("IST"))
        .date_naive()
}

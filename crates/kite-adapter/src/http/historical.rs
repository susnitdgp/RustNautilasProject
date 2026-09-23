//! Read-only historical OHLCV input for the pinned CRUDEOIL contract.
use anyhow::{Result, ensure};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Timelike};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Interval {
    #[serde(rename = "3minute")]
    ThreeMinute,
    #[serde(rename = "5minute")]
    FiveMinute,
}
impl Interval {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ThreeMinute => "3minute",
            Self::FiveMinute => "5minute",
        }
    }
    pub const fn minutes(self) -> u64 {
        match self {
            Self::ThreeMinute => 3,
            Self::FiveMinute => 5,
        }
    }
    pub const fn nanoseconds(self) -> u64 {
        self.minutes() * 60_000_000_000
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candle {
    pub timestamp: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: u64,
    pub oi: u64,
}
impl Candle {
    pub fn time(&self) -> Result<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(&self.timestamp)
            .or_else(|_| DateTime::parse_from_str(&self.timestamp, "%Y-%m-%dT%H:%M:%S%z"))
            .map_err(|_| anyhow::anyhow!("Invalid historical candle timestamp: {}", self.timestamp))
    }
}
#[derive(Deserialize)]
struct Response {
    candles: Vec<(String, f64, f64, f64, f64, u64, u64)>,
}
pub async fn fetch(token: u32, date: NaiveDate) -> Result<Vec<Candle>> {
    fetch_window(token, date, 7).await
}
pub async fn fetch_window(token: u32, date: NaiveDate, days: i64) -> Result<Vec<Candle>> {
    fetch_window_for(token, date, days, Interval::FiveMinute).await
}
pub async fn fetch_window_for(
    token: u32,
    date: NaiveDate,
    days: i64,
    interval: Interval,
) -> Result<Vec<Candle>> {
    ensure!(
        (1..=30).contains(&days),
        "Historical lookback must be 1..30 calendar days"
    );
    ensure!(token > 0, "Invalid instrument token");
    let credentials = crate::credentials::redis::load_from_env()?;
    let client = super::authenticated::ReadClient::new(&credentials)?;
    let from = date
        .checked_sub_signed(Duration::days(days))
        .ok_or_else(|| anyhow::anyhow!("Date overflow"))?;
    let response: Response = client
        .historical(token, from, date, interval.as_str())
        .await?;
    let candles: Vec<_> = response
        .candles
        .into_iter()
        .map(|(timestamp, open, high, low, close, volume, oi)| Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume,
            oi,
        })
        .collect();
    validate_for(&candles, interval)?;
    Ok(candles)
}
pub fn validate(candles: &[Candle]) -> Result<()> {
    validate_for(candles, Interval::FiveMinute)
}
pub fn validate_for(candles: &[Candle], interval: Interval) -> Result<()> {
    ensure!(!candles.is_empty(), "Historical API returned no candles");
    let mut previous = None;
    for c in candles {
        let t = c.time()?;
        ensure!(
            t.offset().local_minus_utc() == 19800,
            "Candle timestamp must use IST +05:30"
        );
        ensure!(
            u64::from(t.minute()) % interval.minutes() == 0
                && t.second() == 0
                && t.nanosecond() == 0,
            "Candle is not aligned to the configured interval"
        );
        ensure!(
            previous.is_none_or(|p| t > p),
            "Duplicate or unordered candle"
        );
        previous = Some(t);
        ensure!(
            [c.open, c.high, c.low, c.close]
                .iter()
                .all(|p| p.is_finite() && *p > 0.0 && p.fract() == 0.0),
            "Invalid CRUDEOIL price/tick size"
        );
        ensure!(
            c.low <= c.open.min(c.close) && c.high >= c.open.max(c.close) && c.low <= c.high,
            "Invalid candle OHLC"
        );
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unordered_misaligned_and_invalid_ohlc() {
        let c = Candle {
            timestamp: "2026-09-15T09:00:00+05:30".into(),
            open: 6000.,
            high: 6010.,
            low: 5990.,
            close: 6001.,
            volume: 10,
            oi: 20,
        };
        assert!(validate(std::slice::from_ref(&c)).is_ok());
        let mut compact = c.clone();
        compact.timestamp = "2026-09-15T09:00:00+0530".into();
        assert_eq!(compact.time().unwrap(), c.time().unwrap());
        assert!(validate(&[c.clone(), c.clone()]).is_err());
        let mut bad = c.clone();
        bad.high = 5999.;
        assert!(validate(&[bad]).is_err());
        let mut bad = c;
        bad.timestamp = "2026-09-15T09:01:00+05:30".into();
        assert!(validate(&[bad]).is_err());
    }
    #[test]
    fn three_minute_validation_is_interval_specific() {
        let candle = Candle {
            timestamp: "2026-09-15T09:03:00+05:30".into(),
            open: 6000.,
            high: 6010.,
            low: 5990.,
            close: 6001.,
            volume: 10,
            oi: 20,
        };
        assert!(validate_for(std::slice::from_ref(&candle), Interval::ThreeMinute).is_ok());
        assert!(validate_for(&[candle], Interval::FiveMinute).is_err());
        assert_eq!(Interval::ThreeMinute.nanoseconds(), 180_000_000_000);
    }
}

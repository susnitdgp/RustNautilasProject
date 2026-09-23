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
    Reader::default()
        .fetch_window_for(token, date, days, interval)
        .await
}

/// Reuse HTTP/TLS connections across live reads. Authentication is still
/// reloaded from Redis on every call, retaining the existing rotation behavior.
#[derive(Default)]
pub struct Reader {
    client: Option<super::authenticated::ReadClient>,
}
impl Reader {
    pub async fn fetch_window_for(
        &mut self,
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
        if let Some(client) = &mut self.client {
            client.refresh_credentials(&credentials)?;
        } else {
            self.client = Some(super::authenticated::ReadClient::new(&credentials)?);
        }
        let client = self.client.as_ref().expect("historical client initialized");
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
}

/// Do not turn authentication rejection or a broker cooldown into rapid retries.
pub fn terminal_read_error(error: &anyhow::Error) -> bool {
    use crate::execution::native_client::outage::ReadFailure;
    matches!(
        error.downcast_ref::<ReadFailure>(),
        Some(ReadFailure::SessionExpired | ReadFailure::RateLimited(_))
    )
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
    fn auth_and_rate_limits_are_not_fast_retryable() {
        use crate::execution::native_client::outage::ReadFailure;
        assert!(terminal_read_error(&anyhow::anyhow!(
            ReadFailure::SessionExpired
        )));
        assert!(terminal_read_error(&anyhow::anyhow!(
            ReadFailure::RateLimited(60_000)
        )));
        assert!(!terminal_read_error(&anyhow::anyhow!(
            ReadFailure::Transient
        )));
    }
    #[tokio::test]
    async fn invalid_reused_reader_input_fails_before_loading_credentials() {
        let mut reader = Reader::default();
        let date = NaiveDate::from_ymd_opt(2026, 9, 23).unwrap();
        assert!(
            reader
                .fetch_window_for(1, date, 0, Interval::ThreeMinute)
                .await
                .unwrap_err()
                .to_string()
                .contains("lookback")
        );
        assert!(
            reader
                .fetch_window_for(0, date, 7, Interval::FiveMinute)
                .await
                .unwrap_err()
                .to_string()
                .contains("token")
        );
        assert!(reader.client.is_none());
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

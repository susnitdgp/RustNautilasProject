//! Read-only historical OHLCV input for the pinned CRUDEOIL contract.
use anyhow::{Result, ensure};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Timelike};
use serde::{Deserialize, Serialize};

/// Candle intervals the Kite historical API actually serves (intraday, up to 30m).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum KiteInterval {
    #[serde(rename = "minute")]
    Minute,
    #[serde(rename = "3minute")]
    ThreeMinute,
    #[serde(rename = "5minute")]
    FiveMinute,
    #[serde(rename = "10minute")]
    TenMinute,
    #[serde(rename = "15minute")]
    FifteenMinute,
    #[serde(rename = "30minute")]
    ThirtyMinute,
}
impl KiteInterval {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minute => "minute",
            Self::ThreeMinute => "3minute",
            Self::FiveMinute => "5minute",
            Self::TenMinute => "10minute",
            Self::FifteenMinute => "15minute",
            Self::ThirtyMinute => "30minute",
        }
    }
    pub const fn minutes(self) -> u32 {
        match self {
            Self::Minute => 1,
            Self::ThreeMinute => 3,
            Self::FiveMinute => 5,
            Self::TenMinute => 10,
            Self::FifteenMinute => 15,
            Self::ThirtyMinute => 30,
        }
    }
    /// The same candle size served natively.
    pub const fn native(self) -> Interval {
        Interval { minutes: self.minutes(), source: self }
    }
}

/// A candle size and the Kite interval its history is fetched at. When the two differ
/// (e.g. 2m from "minute"), fetched candles are combined by [`aggregate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    minutes: u32,
    source: KiteInterval,
}
#[allow(non_upper_case_globals)]
impl Interval {
    pub const OneMinute: Self = KiteInterval::Minute.native();
    pub const ThreeMinute: Self = KiteInterval::ThreeMinute.native();
    pub const FiveMinute: Self = KiteInterval::FiveMinute.native();

    /// `minutes`-minute candles built from `source` history. Sizes must divide 30 so
    /// buckets line up with the IST clock (+05:30) and the MCX session open.
    pub fn built_from(minutes: u32, source: KiteInterval) -> Result<Self> {
        ensure!(
            minutes > 0 && 30 % minutes == 0,
            "candle minutes must be one of 1, 2, 3, 5, 6, 10, 15, 30 (got {minutes})"
        );
        ensure!(
            minutes.is_multiple_of(source.minutes()),
            "{minutes}-minute candles cannot be built from Kite \"{}\" candles",
            source.as_str()
        );
        Ok(Self { minutes, source })
    }
    /// Label such as "2minute" (logs only; the API is called with [`Self::api_interval`]).
    pub fn label(self) -> String {
        if self.minutes == 1 { "minute".into() } else { format!("{}minute", self.minutes) }
    }
    pub const fn minutes(self) -> u64 {
        self.minutes as u64
    }
    /// The interval actually requested from the Kite historical API.
    pub const fn api_interval(self) -> KiteInterval {
        self.source
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
        let api = interval.api_interval();
        let response: Response = client
            .historical(token, from, date, api.as_str())
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
        validate_for(&candles, api.native())?;
        let candles = if api.native() == interval { candles } else { aggregate(&candles, interval)? };
        validate_for(&candles, interval)?;
        Ok(candles)
    }
}

/// Builds `interval` candles from validated finer candles. Buckets are aligned to the
/// clock (2m → :00, :02, …; IST's +05:30 is a whole number of buckets). A bucket with a
/// missing minute (no trades) is built from the minutes that exist.
pub fn aggregate(candles: &[Candle], interval: Interval) -> Result<Vec<Candle>> {
    let step = i64::try_from(interval.minutes() * 60)?;
    let mut out: Vec<Candle> = Vec::with_capacity(candles.len() / 2 + 1);
    let mut current: Option<i64> = None;
    for c in candles {
        let t = c.time()?;
        let bucket = t.timestamp().div_euclid(step) * step;
        if current == Some(bucket) {
            let agg = out.last_mut().expect("open bucket");
            agg.high = agg.high.max(c.high);
            agg.low = agg.low.min(c.low);
            agg.close = c.close;
            agg.volume += c.volume;
            agg.oi = c.oi;
            continue;
        }
        let start = DateTime::from_timestamp(bucket, 0)
            .ok_or_else(|| anyhow::anyhow!("Candle bucket out of range"))?
            .with_timezone(t.offset());
        out.push(Candle { timestamp: start.to_rfc3339(), ..c.clone() });
        current = Some(bucket);
    }
    Ok(out)
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
                .all(|p| p.is_finite() && *p > 0.0),
            "Invalid historical price"
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
    fn one(ts: &str, o: f64, h: f64, l: f64, c: f64, v: u64) -> Candle {
        Candle { timestamp: ts.into(), open: o, high: h, low: l, close: c, volume: v, oi: v }
    }
    #[test]
    fn two_minute_candles_are_built_from_one_minute_history() {
        let two = Interval::built_from(2, KiteInterval::Minute).unwrap();
        assert_eq!((two.minutes(), two.api_interval(), two.label().as_str()), (2, KiteInterval::Minute, "2minute"));
        let src = [
            one("2026-10-09T09:00:00+0530", 100., 105., 99., 104., 10),
            one("2026-10-09T09:01:00+0530", 104., 108., 101., 102., 5),
            one("2026-10-09T09:02:00+0530", 102., 103., 97., 98., 7),
            // 09:03 had no trades
            one("2026-10-09T09:04:00+0530", 98., 99., 96., 97., 3),
        ];
        validate_for(&src, KiteInterval::Minute.native()).unwrap();
        let out = aggregate(&src, two).unwrap();
        validate_for(&out, two).unwrap();
        let got: Vec<_> =
            out.iter().map(|c| (c.time().unwrap().format("%H:%M").to_string(), c.open, c.high, c.low, c.close, c.volume, c.oi)).collect();
        assert_eq!(
            got,
            vec![
                ("09:00".into(), 100., 108., 99., 102., 15, 5),
                ("09:02".into(), 102., 103., 97., 98., 7, 7),
                ("09:04".into(), 98., 99., 96., 97., 3, 3),
            ]
        );
        assert_eq!(out[0].time().unwrap().offset().local_minus_utc(), 19_800);
    }
    #[test]
    fn candle_sizes_must_fit_the_clock_and_the_source() {
        for m in [1, 2, 3, 5, 6, 10, 15, 30] {
            assert!(Interval::built_from(m, KiteInterval::Minute).is_ok(), "{m}");
        }
        assert!(Interval::built_from(4, KiteInterval::Minute).is_err());
        assert!(Interval::built_from(60, KiteInterval::Minute).is_err());
        assert!(Interval::built_from(2, KiteInterval::ThreeMinute).is_err());
        assert!(Interval::built_from(15, KiteInterval::FiveMinute).is_ok());
        assert_eq!(Interval::built_from(5, KiteInterval::FiveMinute).unwrap(), Interval::FiveMinute);
    }
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

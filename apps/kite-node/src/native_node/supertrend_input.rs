//! Historical input validation and causal bar/open-quote replay.
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use kite_adapter::http::historical::Candle;
use nautilus_model::data::{BarType, Data};
use serde::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize)]
pub struct Input {
    pub instrument_id: String,
    pub instrument_token: u32,
    pub interval: String,
    pub source: String,
    pub candles: Vec<Candle>,
}
pub fn load(date: NaiveDate, path: Option<&str>) -> Result<Input> {
    load_window(date, path, 7)
}
pub fn load_window(date: NaiveDate, path: Option<&str>, days: i64) -> Result<Input> {
    if let Some(path) = path {
        ensure!(
            std::fs::metadata(path)?.len() <= 8 * 1024 * 1024,
            "Historical file exceeds size limit"
        );
        return Ok(serde_json::from_slice(&std::fs::read(path)?)?);
    }
    let config = kite_adapter::config::Config::parse(&std::fs::read_to_string(
        "config/crudeoil-september.toml",
    )?)?;
    let master = kite_adapter::http::instruments::download()?;
    let report = kite_adapter::preflight::run(&config, master.as_slice(), date)?;
    let candles = tokio::runtime::Runtime::new()?.block_on(
        kite_adapter::http::historical::fetch_window(report.instrument_token, date, days),
    )?;
    Ok(Input {
        instrument_id: report.instrument_id,
        instrument_token: report.instrument_token,
        interval: "5minute".into(),
        source: "kite_historical_api".into(),
        candles,
    })
}
pub fn bounds(date: NaiveDate) -> Result<(u64, u64)> {
    let parse = |time: &str| -> Result<u64> {
        Ok(u64::try_from(
            chrono::DateTime::parse_from_rfc3339(&format!("{date}T{time}+05:30"))?
                .timestamp_nanos_opt()
                .ok_or_else(|| anyhow::anyhow!("Timestamp overflow"))?,
        )?)
    };
    Ok((parse("09:00:00")?, parse("23:30:00")?))
}
pub fn validate(input: &Input, date: NaiveDate) -> Result<usize> {
    super::vwap_input::validate(input, date)
}
pub fn replay(input: &Input, date: NaiveDate, bar_type: BarType) -> Result<Vec<Data>> {
    super::vwap_input::replay(input, date, bar_type)
}
#[cfg(test)]
pub fn fixture() -> Input {
    let mut candles = Vec::new();
    for day in [14, 15] {
        let date = NaiveDate::from_ymd_opt(2026, 9, day).unwrap();
        for i in 0..174 {
            let minute = 540 + i * 5;
            let p = 6000.
                + if (i / 20) % 2 == 0 {
                    (i % 20) as f64 * 10.
                } else {
                    200. - (i % 20) as f64 * 10.
                };
            candles.push(Candle {
                timestamp: format!("{date}T{:02}:{:02}:00+05:30", minute / 60, minute % 60),
                open: p,
                high: p + 5.,
                low: p - 5.,
                close: p + 1.,
                volume: 100,
                oi: 1000,
            });
        }
    }
    Input {
        instrument_id: "CRUDEOIL26SEPFUT.MCX".into(),
        instrument_token: 144870151,
        interval: "5minute".into(),
        source: "synthetic_test_fixture".into(),
        candles,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_coverage_and_never_delivers_close_before_open() {
        let mut input = fixture();
        let date = NaiveDate::from_ymd_opt(2026, 9, 15).unwrap();
        let bars: BarType = "CRUDEOIL26SEPFUT.MCX-5-MINUTE-LAST-EXTERNAL"
            .parse()
            .unwrap();
        let data = replay(&input, date, bars).unwrap();
        let (start, _) = bounds(date).unwrap();
        let first = data
            .iter()
            .position(|d| matches!(d, Data::Quote(_)))
            .unwrap();
        assert!(matches!(data[first],Data::Quote(q) if q.ts_event.as_u64()==start+2));
        assert!(matches!(data[first+2],Data::Bar(b) if b.ts_event.as_u64()==start+300_000_000_000));
        input.candles.remove(180);
        assert!(validate(&input, date).is_err());
    }
}

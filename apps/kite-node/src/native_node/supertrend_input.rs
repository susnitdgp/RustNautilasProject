//! Historical input validation and causal bar/open-quote replay.
use anyhow::{Result, ensure};
use chrono::{NaiveDate, Timelike};
use kite_adapter::http::historical::Candle;
use nautilus_model::{
    data::{Bar, BarType, Data, QuoteTick},
    types::{Price, Quantity},
};
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
pub fn validate(input: &Input, date: NaiveDate) -> Result<()> {
    ensure!(
        date >= NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
            && date <= NaiveDate::from_ymd_opt(2026, 9, 21).unwrap(),
        "Date outside configured September 2026 contract scope"
    );
    ensure!(
        input.instrument_id == "CRUDEOIL26SEPFUT.MCX"
            && input.instrument_token == 144870151
            && input.interval == "5minute",
        "Historical instrument/interval mismatch"
    );
    kite_adapter::http::historical::validate(&input.candles)?;
    let target: Vec<_> = input
        .candles
        .iter()
        .filter(|c| c.time().is_ok_and(|t| t.date_naive() == date))
        .collect();
    ensure!(
        target.len() == 174,
        "Expected 174 five-minute candles for 09:00–23:30 IST; received {}",
        target.len()
    );
    let mut warmup = 0;
    for c in &input.candles {
        let t = c.time()?;
        ensure!(t.date_naive() <= date, "Input contains future candles");
        if t.date_naive() < date {
            warmup += 1;
        }
    }
    ensure!(
        warmup >= 100,
        "At least 100 prior-session warmup candles are required"
    );
    for (i, c) in target.iter().enumerate() {
        let t = c.time()?;
        ensure!(
            t.hour() * 60 + t.minute() == 540 + i as u32 * 5,
            "Requested session has missing or out-of-session candles"
        );
    }
    Ok(())
}
pub fn replay(input: &Input, date: NaiveDate, bar_type: BarType) -> Result<Vec<Data>> {
    validate(input, date)?;
    let mut data = Vec::new();
    for c in &input.candles {
        let t = c.time()?;
        let start = u64::try_from(
            t.timestamp_nanos_opt()
                .ok_or_else(|| anyhow::anyhow!("Timestamp overflow"))?,
        )?;
        let end = start + 300_000_000_000;
        if t.date_naive() == date {
            // Two ordered quotes permit reducing exit then entry after its fill; no doubled reversal order.
            for offset in [2, 3] {
                data.push(quote(bar_type, c.open, start + offset));
            }
        }
        data.push(Data::Bar(Bar::new(
            bar_type,
            Price::new(c.open, 0),
            Price::new(c.high, 0),
            Price::new(c.low, 0),
            Price::new(c.close, 0),
            Quantity::from(c.volume),
            (end).into(),
            (end).into(),
        )));
    }
    let (_, end) = bounds(date)?;
    let close = input.candles.last().expect("validated").close;
    data.push(quote(bar_type, close, end + 2));
    data.sort_by_key(|d| match d {
        Data::Bar(b) => b.ts_init.as_u64(),
        Data::Quote(q) => q.ts_init.as_u64(),
        _ => unreachable!(),
    });
    Ok(data)
}
fn quote(bar_type: BarType, price: f64, ts: u64) -> Data {
    Data::Quote(QuoteTick::new(
        bar_type.instrument_id(),
        Price::new(price, 0),
        Price::new(price, 0),
        Quantity::from(1000),
        Quantity::from(1000),
        ts.into(),
        ts.into(),
    ))
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

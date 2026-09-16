//! Explicit September MCX session calendar; no silently skipped incomplete sessions.
use super::supertrend_input::Input;
use anyhow::{Result, ensure};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use nautilus_model::{
    data::{Bar, BarType, Data, QuoteTick},
    types::{Price, Quantity},
};
pub fn bounds(date: NaiveDate) -> Result<(u64, u64)> {
    ensure!(
        date >= NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
            && date <= NaiveDate::from_ymd_opt(2026, 9, 21).unwrap(),
        "Session calendar supports September 1–21, 2026 only"
    );
    ensure!(
        !matches!(date.weekday(), Weekday::Sat | Weekday::Sun),
        "No weekend session configured"
    );
    let (start, end) = super::supertrend_input::bounds(date)?;
    // Official MCX calendar: Ganesh Chaturthi, morning closed, evening open.
    Ok((
        if date.day() == 14 {
            start + 8 * 3_600_000_000_000
        } else {
            start
        },
        end,
    ))
}
pub fn dates(end: NaiveDate) -> Result<Vec<NaiveDate>> {
    let mut days = Vec::new();
    let mut day = end;
    while days.len() < 7 {
        if !matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
            bounds(day)?;
            days.push(day);
        }
        day = day
            .checked_sub_signed(Duration::days(1))
            .ok_or_else(|| anyhow::anyhow!("Date overflow"))?;
    }
    ensure!(
        days.first() == Some(&end),
        "End date must be a trading session"
    );
    days.reverse();
    Ok(days)
}
pub fn validate(input: &Input, date: NaiveDate) -> Result<usize> {
    ensure!(
        input.instrument_id == "CRUDEOIL26SEPFUT.MCX"
            && input.instrument_token == 144870151
            && input.interval == "5minute",
        "Wrong instrument or interval"
    );
    kite_adapter::http::historical::validate(&input.candles)?;
    let (start, end) = bounds(date)?;
    let expected = ((end - start) / 300_000_000_000) as usize;
    let mut count = 0;
    let mut warmup = 0;
    for c in &input.candles {
        let t = c.time()?;
        ensure!(t.date_naive() <= date, "Future candle in session input");
        if t.date_naive() < date {
            warmup += 1;
            continue;
        }
        let ts = u64::try_from(
            t.timestamp_nanos_opt()
                .ok_or_else(|| anyhow::anyhow!("Timestamp overflow"))?,
        )?;
        ensure!(
            ts == start + count as u64 * 300_000_000_000,
            "Missing or out-of-session candle on {date} at {}",
            c.timestamp
        );
        count += 1;
    }
    ensure!(
        count == expected,
        "Incomplete {date} session: expected {expected} bars, received {count}"
    );
    ensure!(warmup >= 100, "Insufficient prior-session warmup");
    Ok(count)
}
pub fn replay(input: &Input, date: NaiveDate, bt: BarType) -> Result<Vec<Data>> {
    validate(input, date)?;
    let mut data = Vec::new();
    for c in &input.candles {
        let t = c.time()?;
        let open = u64::try_from(
            t.timestamp_nanos_opt()
                .ok_or_else(|| anyhow::anyhow!("Timestamp overflow"))?,
        )?;
        if t.date_naive() == date {
            for offset in [2, 3] {
                data.push(quote(bt, c.open, open + offset));
            }
        }
        let close = open + 300_000_000_000;
        data.push(Data::Bar(Bar::new(
            bt,
            Price::new(c.open, 0),
            Price::new(c.high, 0),
            Price::new(c.low, 0),
            Price::new(c.close, 0),
            Quantity::from(c.volume),
            close.into(),
            close.into(),
        )));
    }
    let (_, end) = bounds(date)?;
    data.push(quote(
        bt,
        input.candles.last().expect("validated").close,
        end + 2,
    ));
    data.sort_by_key(|d| match d {
        Data::Bar(b) => b.ts_init.as_u64(),
        Data::Quote(q) => q.ts_init.as_u64(),
        _ => unreachable!(),
    });
    Ok(data)
}
fn quote(bt: BarType, p: f64, ts: u64) -> Data {
    Data::Quote(QuoteTick::new(
        bt.instrument_id(),
        Price::new(p, 0),
        Price::new(p, 0),
        1000.into(),
        1000.into(),
        ts.into(),
        ts.into(),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn includes_seven_sessions_and_evening_holiday_without_skipping_missing_days() {
        let d = NaiveDate::from_ymd_opt(2026, 9, 15).unwrap();
        let days = dates(d).unwrap();
        assert_eq!(days.len(), 7);
        assert_eq!(days[0].day(), 7);
        let (a, b) = bounds(days[5]).unwrap();
        assert_eq!((b - a) / 300_000_000_000, 78);
        let mut data = super::super::supertrend_input::fixture();
        assert_eq!(validate(&data, d).unwrap(), 174);
        data.candles.remove(180);
        assert!(validate(&data, d).is_err());
    }
}

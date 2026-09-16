//! Explicit August–September MCX session calendar; no silently skipped incomplete sessions.
use super::supertrend_input::Input;
use anyhow::{Result, ensure};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use nautilus_model::{
    data::{Bar, BarType, Data, QuoteTick},
    types::{Price, Quantity},
};
pub fn bounds(date: NaiveDate) -> Result<(u64, u64)> {
    ensure!(
        date >= NaiveDate::from_ymd_opt(2026, 8, 17).unwrap()
            && date <= NaiveDate::from_ymd_opt(2026, 9, 21).unwrap(),
        "Session calendar supports August 17–September 21, 2026 only"
    );
    ensure!(
        !matches!(date.weekday(), Weekday::Sat | Weekday::Sun),
        "No weekend session configured"
    );
    let (start, end) = super::supertrend_input::bounds(date)?;
    // Official MCX calendar: Ganesh Chaturthi, morning closed, evening open.
    Ok((
        if date == NaiveDate::from_ymd_opt(2026, 9, 14).unwrap() {
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
pub fn range(start: NaiveDate, end: NaiveDate) -> Result<Vec<NaiveDate>> {
    ensure!(start <= end, "Start must precede end");
    bounds(start)?;
    bounds(end)?;
    let mut days = Vec::new();
    let mut day = start;
    while day <= end {
        if !matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
            bounds(day)?;
            days.push(day);
        }
        day = day
            .checked_add_signed(Duration::days(1))
            .ok_or_else(|| anyhow::anyhow!("Date overflow"))?;
    }
    Ok(days)
}
pub fn validate(input: &Input, date: NaiveDate) -> Result<usize> {
    ensure!(
        input.interval == "5minute",
        "VWAP strategy requires five-minute bars"
    );
    validate_interval(input, date)
}
pub fn validate_interval(input: &Input, date: NaiveDate) -> Result<usize> {
    let step = step_ns(&input.interval)?;
    ensure!(
        input.instrument_id == "CRUDEOIL26SEPFUT.MCX"
            && input.instrument_token == 144870151
            && matches!(input.interval.as_str(), "5minute" | "10minute"),
        "Wrong instrument or interval"
    );
    kite_adapter::http::historical::validate(&input.candles)?;
    let (start, end) = bounds(date)?;
    let expected = ((end - start) / step) as usize;
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
            ts == start + count as u64 * step,
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
    replay_interval(input, date, bt)
}
pub fn step_ns(interval: &str) -> Result<u64> {
    match interval {
        "5minute" => Ok(300_000_000_000),
        "10minute" => Ok(600_000_000_000),
        _ => anyhow::bail!("Only five or ten-minute bars are supported"),
    }
}
pub fn replay_interval(input: &Input, date: NaiveDate, bt: BarType) -> Result<Vec<Data>> {
    validate_interval(input, date)?;
    let step = step_ns(&input.interval)?;
    let expected: BarType = format!(
        "CRUDEOIL26SEPFUT.MCX-{}-MINUTE-LAST-EXTERNAL",
        step / 60_000_000_000
    )
    .parse()?;
    ensure!(bt == expected, "Bar type does not match interval");
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
        let close = open + step;
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
    fn thirty_calendar_days_cover_all_twenty_two_sessions() {
        let start = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 9, 15).unwrap();
        let days = range(start, end).unwrap();
        assert_eq!(days.len(), 22);
        assert_eq!(days[0], start);
        assert_eq!(days[21], end);
        let bars: u64 = days
            .iter()
            .map(|d| {
                let (a, b) = bounds(*d).unwrap();
                (b - a) / 300_000_000_000
            })
            .sum();
        assert_eq!(bars, 3732);
        assert!(range(end, start).is_err());
        assert!(bounds(NaiveDate::from_ymd_opt(2026, 8, 16).unwrap()).is_err());
    }
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

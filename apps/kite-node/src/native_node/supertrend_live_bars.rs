//! Completed-candle selection, revision/gap detection and native bar conversion.
use anyhow::{Result, ensure};
use kite_adapter::http::historical::{Candle, Interval};
use nautilus_model::{
    data::{Bar, BarType},
    types::{Price, Quantity},
};
#[cfg(test)]
use std::collections::BTreeMap;
#[cfg(test)]
const STEP: u64 = 300_000_000_000;
#[cfg(test)]
pub fn close(c: &Candle) -> Result<u64> {
    close_for(c, Interval::FiveMinute)
}
pub fn close_for(c: &Candle, interval: Interval) -> Result<u64> {
    Ok(u64::try_from(
        c.time()?
            .timestamp_nanos_opt()
            .ok_or_else(|| anyhow::anyhow!("Timestamp overflow"))?,
    )? + interval.nanoseconds())
}
#[cfg(test)]
pub fn completed(candles: Vec<Candle>, now: u64) -> Result<Vec<Candle>> {
    completed_for(candles, now, Interval::FiveMinute)
}
pub fn completed_for(candles: Vec<Candle>, now: u64, interval: Interval) -> Result<Vec<Candle>> {
    kite_adapter::http::historical::validate_for(&candles, interval)?;
    let mut out = Vec::new();
    for c in candles {
        if close_for(&c, interval)? <= now.saturating_sub(2_000_000_000) {
            out.push(c);
        }
    }
    ensure!(!out.is_empty(), "No completed candles");
    Ok(out)
}
pub fn bar_for(c: &Candle, bt: BarType, received: u64, interval: Interval) -> Result<Bar> {
    Ok(Bar::new(
        bt,
        Price::new(c.open, 0),
        Price::new(c.high, 0),
        Price::new(c.low, 0),
        Price::new(c.close, 0),
        Quantity::from(c.volume),
        close_for(c, interval)?.into(),
        received.into(),
    ))
}
#[cfg(test)]
pub struct Tracker {
    known: BTreeMap<u64, Candle>,
    last: u64,
}
#[cfg(test)]
impl Tracker {
    pub fn new(candles: &[Candle]) -> Result<Self> {
        ensure!(candles.len() >= 100, "Insufficient indicator warmup");
        let mut known = BTreeMap::new();
        for c in candles {
            known.insert(close(c)?, c.clone());
        }
        let last = *known
            .last_key_value()
            .ok_or_else(|| anyhow::anyhow!("No warmup"))?
            .0;
        Ok(Self { known, last })
    }
    pub fn append(&mut self, candles: Vec<Candle>) -> Result<Vec<Candle>> {
        let mut output = Vec::new();
        let mut last = self.last;
        for c in &candles {
            let ts = close(c)?;
            if let Some(old) = self.known.get(&ts) {
                ensure!(
                    (old.open, old.high, old.low, old.close, old.volume, old.oi)
                        == (c.open, c.high, c.low, c.close, c.volume, c.oi),
                    "Previously completed candle revised at {}: OHLCV/OI {:?} -> {:?}",
                    c.timestamp,
                    (old.open, old.high, old.low, old.close, old.volume, old.oi),
                    (c.open, c.high, c.low, c.close, c.volume, c.oi)
                );
            } else if ts > self.last {
                ensure!(ts == last + STEP, "Gap in completed live candles");
                output.push(c.clone());
                last = ts;
            }
        }
        for c in &output {
            self.known.insert(close(c)?, c.clone());
        }
        self.last = last;
        Ok(output)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_bars_duplicates_gaps_and_revisions_are_handled() {
        let all = super::super::supertrend_input::fixture().candles;
        let cutoff = close(&all[100]).unwrap() + 1_000_000_000;
        assert_eq!(completed(all.clone(), cutoff).unwrap().len(), 100);
        let mut t = Tracker::new(&all[..100]).unwrap();
        assert!(t.append(all[..100].to_vec()).unwrap().is_empty());
        assert!(t.append(vec![all[101].clone()]).is_err());
        assert_eq!(t.append(vec![all[100].clone()]).unwrap().len(), 1);
        let mut revised = all[100].clone();
        revised.volume += 1;
        assert!(t.append(vec![revised]).is_err());
    }
}

#[cfg(test)]
pub fn validate_warmup(
    candles: &[Candle],
    date: chrono::NaiveDate,
    now: u64,
    calendar: &super::session_calendar::Calendar,
) -> Result<()> {
    validate_warmup_for(candles, date, now, calendar, Interval::FiveMinute)
}
pub fn validate_warmup_for(
    candles: &[Candle],
    date: chrono::NaiveDate,
    now: u64,
    calendar: &super::session_calendar::Calendar,
    interval: Interval,
) -> Result<()> {
    let first = candles
        .first()
        .ok_or_else(|| anyhow::anyhow!("No warmup"))?
        .time()?
        .date_naive();
    let mut index = 0;
    for day in calendar.range(first, date)? {
        let (start, end) = calendar.bounds(day)?;
        let cutoff = if day == date {
            now.saturating_sub(2_000_000_000).min(end)
        } else {
            end
        };
        let mut expected = start + interval.nanoseconds();
        while expected <= cutoff {
            let c = candles
                .get(index)
                .ok_or_else(|| anyhow::anyhow!("Missing completed warmup candle"))?;
            ensure!(
                close_for(c, interval)? == expected,
                "Gap or out-of-session warmup candle"
            );
            index += 1;
            expected += interval.nanoseconds();
        }
    }
    ensure!(index == candles.len(), "Unexpected warmup candles");
    Ok(())
}

#[test]
fn warmup_requires_complete_sessions_and_latest_closed_candle() {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
    let calendar = super::session_calendar::fixture();
    let (start, _) = super::vwap_input::bounds(date).unwrap();
    let now = start + 3 * STEP + 2_000_000_000;
    let (previous, end) = super::vwap_input::bounds(date.pred_opt().unwrap()).unwrap();
    let mut candles = Vec::new();
    for ts in (previous..end)
        .step_by(STEP as usize)
        .chain((start..start + 3 * STEP).step_by(STEP as usize))
    {
        candles.push(Candle {
            timestamp: chrono::DateTime::from_timestamp_nanos(ts as i64)
                .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
                .to_rfc3339(),
            open: 100.,
            high: 101.,
            low: 99.,
            close: 100.,
            volume: 10,
            oi: 10,
        });
    }
    assert!(validate_warmup(&candles, date, now, &calendar).is_ok());
    assert!(validate_warmup(&candles[..candles.len() - 1], date, now, &calendar).is_err());
    candles.remove(12);
    assert!(validate_warmup(&candles, date, now, &calendar).is_err());
}

#[test]
fn three_minute_completed_bars_and_warmup_use_json_interval_step() {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
    let calendar = super::session_calendar::fixture();
    let (start, _) = calendar.bounds(date).unwrap();
    let interval = Interval::ThreeMinute;
    let now = start + 3 * interval.nanoseconds() + 2_000_000_000;
    let mut candles = Vec::new();
    for ts in (start..start + 3 * interval.nanoseconds()).step_by(interval.nanoseconds() as usize) {
        candles.push(Candle {
            timestamp: chrono::DateTime::from_timestamp_nanos(ts as i64)
                .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
                .to_rfc3339(),
            open: 100.,
            high: 101.,
            low: 99.,
            close: 100.,
            volume: 10,
            oi: 10,
        });
    }
    let completed = completed_for(candles.clone(), now, interval).unwrap();
    assert_eq!(completed.len(), 3);
    assert_eq!(
        close_for(&completed[0], interval).unwrap(),
        start + interval.nanoseconds()
    );
    assert!(validate_warmup_for(&candles, date, now, &calendar, interval).is_ok());
}

//! Merge corrected broker history atomically, before rebuilding indicators.
use super::supertrend_live_bars::{close, validate_warmup};
use anyhow::{Result, ensure};
use kite_adapter::http::historical::Candle;
use std::collections::BTreeMap;
pub struct History {
    candles: BTreeMap<u64, Candle>,
}
pub struct Update {
    pub all: Vec<Candle>,
    pub new: Vec<Candle>,
    pub revised: usize,
}
impl History {
    pub fn new(candles: &[Candle]) -> Result<Self> {
        Ok(Self {
            candles: candles
                .iter()
                .map(|c| Ok((close(c)?, c.clone())))
                .collect::<Result<_>>()?,
        })
    }
    pub fn update(
        &mut self,
        incoming: Vec<Candle>,
        date: chrono::NaiveDate,
        now: u64,
    ) -> Result<Update> {
        kite_adapter::http::historical::validate(&incoming)?;
        let mut merged = self.candles.clone();
        let mut revised = 0;
        let mut new = Vec::new();
        let last = *merged
            .last_key_value()
            .ok_or_else(|| anyhow::anyhow!("Empty history"))?
            .0;
        for c in incoming {
            let ts = close(&c)?;
            ensure!(
                ts <= now.saturating_sub(2_000_000_000),
                "Unfinished correction"
            );
            if let Some(old) = merged.get(&ts) {
                // OI does not enter this strategy's indicators.
                if (old.open, old.high, old.low, old.close, old.volume)
                    != (c.open, c.high, c.low, c.close, c.volume)
                {
                    revised += 1;
                }
            } else if ts > last {
                new.push(c.clone());
            }
            merged.insert(ts, c);
        }
        let all: Vec<_> = merged.values().cloned().collect();
        validate_warmup(&all, date, now)?;
        self.candles = merged;
        Ok(Update { all, new, revised })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Vec<Candle>, chrono::NaiveDate, u64) {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
        let (a, b) = super::super::vwap_input::bounds(date).unwrap();
        let rows = (a..b)
            .step_by(300_000_000_000usize)
            .map(|ts| Candle {
                timestamp: chrono::DateTime::from_timestamp_nanos(ts as i64)
                    .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
                    .to_rfc3339(),
                open: 100.,
                high: 102.,
                low: 99.,
                close: 101.,
                volume: 10,
                oi: 10,
            })
            .collect();
        (rows, date, b + 2_000_000_000)
    }
    #[test]
    fn corrections_rebuild_once_and_gaps_do_not_commit_partial_history() {
        let (rows, date, now) = fixture();
        let mut h = History::new(&rows[..100]).unwrap();
        let mut missing = rows[100..].to_vec();
        missing.remove(2);
        assert!(h.update(missing, date, now).is_err());
        let u = h.update(rows[100..].to_vec(), date, now).unwrap();
        assert_eq!(u.new.len(), 74);
        let mut revised = rows.clone();
        revised[50].volume += 1;
        assert_eq!(h.update(revised.clone(), date, now).unwrap().revised, 1);
        assert_eq!(h.update(revised.clone(), date, now).unwrap().revised, 0);
        revised[50].oi += 1;
        assert_eq!(h.update(revised, date, now).unwrap().revised, 0);
    }
}

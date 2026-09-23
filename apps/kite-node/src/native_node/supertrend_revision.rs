//! Merge corrected broker history atomically, before rebuilding indicators.
use super::supertrend_live_bars::{close_for, validate_warmup_for};
use anyhow::{Result, ensure};
use kite_adapter::http::historical::{Candle, Interval};
use std::collections::BTreeMap;
pub struct History {
    candles: BTreeMap<u64, Candle>,
    calendar: super::session_calendar::Calendar,
    interval: Interval,
    volume_sensitive: bool,
}
pub struct Update {
    pub all: Vec<Candle>,
    pub new: Vec<Candle>,
    pub revised: usize,
    pub price_revised: usize,
    pub volume_only_revised: usize,
    pub revision_samples: Vec<String>,
}
impl History {
    #[cfg(test)]
    pub fn new(candles: &[Candle], calendar: super::session_calendar::Calendar) -> Result<Self> {
        Self::new_for(candles, calendar, Interval::FiveMinute)
    }
    pub fn new_for(
        candles: &[Candle],
        calendar: super::session_calendar::Calendar,
        interval: Interval,
    ) -> Result<Self> {
        Ok(Self {
            calendar,
            interval,
            volume_sensitive: true,
            candles: candles
                .iter()
                .map(|c| Ok((close_for(c, interval)?, c.clone())))
                .collect::<Result<_>>()?,
        })
    }
    /// Price-only strategies need no indicator rebuild for volume-only edits.
    /// Corrected volume is still merged; price corrections always rebuild.
    pub fn with_volume_sensitive(mut self, sensitive: bool) -> Self {
        self.volume_sensitive = sensitive;
        self
    }
    pub fn latest_close(&self) -> u64 {
        self.candles.last_key_value().map_or(0, |(close, _)| *close)
    }
    pub fn update(
        &mut self,
        incoming: Vec<Candle>,
        date: chrono::NaiveDate,
        now: u64,
    ) -> Result<Update> {
        kite_adapter::http::historical::validate_for(&incoming, self.interval)?;
        let mut merged = self.candles.clone();
        let mut revised = 0;
        let mut price_revised = 0;
        let mut volume_only_revised = 0;
        let mut revision_samples = Vec::new();
        let mut new = Vec::new();
        let last = *merged
            .last_key_value()
            .ok_or_else(|| anyhow::anyhow!("Empty history"))?
            .0;
        for c in incoming {
            let ts = close_for(&c, self.interval)?;
            ensure!(
                ts <= now.saturating_sub(super::supertrend_bar_timing::COMPLETION_GRACE_NS),
                "Unfinished correction"
            );
            if let Some(old) = merged.get(&ts) {
                // OI is unused. Keep price and volume correction counts separate.
                let price_changed =
                    (old.open, old.high, old.low, old.close) != (c.open, c.high, c.low, c.close);
                let volume_changed = old.volume != c.volume;
                price_revised += usize::from(price_changed);
                volume_only_revised += usize::from(!price_changed && volume_changed);
                revised += usize::from(price_changed || (self.volume_sensitive && volume_changed));
                if (price_changed || volume_changed) && revision_samples.len() < 8 {
                    revision_samples.push(format!(
                        "{}: OHLC={} volume={}",
                        c.timestamp, price_changed, volume_changed
                    ));
                }
            } else if ts > last {
                new.push(c.clone());
            }
            merged.insert(ts, c);
        }
        let all: Vec<_> = merged.values().cloned().collect();
        validate_warmup_for(&all, date, now, &self.calendar, self.interval)?;
        self.candles = merged;
        Ok(Update {
            all,
            new,
            revised,
            price_revised,
            volume_only_revised,
            revision_samples,
        })
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
    fn october_history_skips_closed_holiday_but_rejects_missing_trading_bars() {
        let calendar = super::super::session_calendar::fixture();
        let first = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let mut rows = Vec::new();
        for day in calendar.range(first, date).unwrap() {
            let (a, b) = calendar.bounds(day).unwrap();
            for ts in (a..b).step_by(300_000_000_000usize) {
                rows.push(Candle {
                    timestamp: chrono::DateTime::from_timestamp_nanos(ts as i64)
                        .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
                        .to_rfc3339(),
                    open: 100.,
                    high: 102.,
                    low: 99.,
                    close: 101.,
                    volume: 10,
                    oi: 10,
                });
            }
        }
        assert_eq!(rows.len(), 348);
        let now = calendar.bounds(date).unwrap().1 + 2_000_000_000;
        let mut history = History::new(&rows[..174], calendar).unwrap();
        let mut missing = rows[174..].to_vec();
        missing.remove(12);
        assert!(history.update(missing, date, now).is_err());
        assert_eq!(
            history
                .update(rows[174..].to_vec(), date, now)
                .unwrap()
                .new
                .len(),
            174
        );
        let mut revised = rows;
        revised[50].volume += 1;
        assert_eq!(history.update(revised, date, now).unwrap().revised, 1);
    }
    #[test]
    fn ribbon_merges_volume_corrections_without_rebuild_or_losing_fresh_bars() {
        let (mut rows, date, now) = fixture();
        let mut history = History::new(&rows[..100], super::super::session_calendar::fixture())
            .unwrap()
            .with_volume_sensitive(false);
        rows[50].volume += 10;
        let update = history.update(rows.clone(), date, now).unwrap();
        assert_eq!(update.revised, 0);
        assert_eq!(update.price_revised, 0);
        assert_eq!(update.volume_only_revised, 1);
        assert_eq!(update.new.len(), 74);
        assert_eq!(update.all[50].volume, rows[50].volume);
        assert!(update.revision_samples[0].contains("OHLC=false volume=true"));
        let repeated = history.update(rows.clone(), date, now).unwrap();
        assert_eq!(repeated.volume_only_revised, 0);
        rows[50].high += 1.;
        rows[50].volume += 1;
        let corrected = history.update(rows, date, now).unwrap();
        assert_eq!(corrected.revised, 1);
        assert_eq!(corrected.price_revised, 1);
        assert_eq!(corrected.volume_only_revised, 0);
    }

    #[test]
    fn ribbon_missing_candle_does_not_commit_a_volume_correction() {
        let (mut rows, date, now) = fixture();
        let mut history = History::new(&rows[..100], super::super::session_calendar::fixture())
            .unwrap()
            .with_volume_sensitive(false);
        rows[50].volume += 10;
        let mut missing = rows.clone();
        missing.remove(120);
        assert!(history.update(missing, date, now).is_err());
        let update = history.update(rows, date, now).unwrap();
        assert_eq!(update.volume_only_revised, 1);
        assert_eq!(update.new.len(), 74);
    }

    #[test]
    fn corrections_rebuild_once_and_gaps_do_not_commit_partial_history() {
        let (rows, date, now) = fixture();
        let mut h = History::new(&rows[..100], super::super::session_calendar::fixture()).unwrap();
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

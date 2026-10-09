//! Live, exchange-timestamped OHLCV (3-minute by default) from Kite full WebSocket packets.
//! Historical API is exclusively for warmup; gaps/reconnections fail closed.
use anyhow::{Result, ensure};
use chrono::{TimeZone, Utc};
use kite_adapter::{http::historical::Candle, mapping::market_data::Snapshot};
#[allow(dead_code)] // 3-minute default used by legacy ws_validation tooling
const DEFAULT_STEP: i64 = 180;
#[derive(Debug, Default)]
pub struct Aggregator {
    current: Option<Current>,
    last_close: i64,
    last_volume: Option<u32>,
    generation: Option<u32>,
    last_exchange_ts: Option<i64>,
    skip_partial: bool,
    step: i64,
    /// `(warm-up close, first full stream bar start)` in epoch seconds, set on the
    /// first tick when candles between them can never come from the stream.
    gap: Option<(i64, i64)>,
}
#[derive(Debug)]
struct Current {
    start: i64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: u64,
    oi: u64,
}
impl Aggregator {
    #[allow(dead_code)] // 3-minute default used by legacy ws_validation tooling
    pub fn new(last_close_ns: u64) -> Self {
        Self::with_step(last_close_ns, DEFAULT_STEP)
    }
    /// Candle length in seconds (300 for 5-minute bars).
    pub fn with_step(last_close_ns: u64, step_seconds: i64) -> Self {
        assert!(step_seconds > 0 && 86_400 % step_seconds == 0, "invalid candle step");
        Self {
            last_close: (last_close_ns / 1_000_000_000) as i64,
            step: step_seconds,
            ..Self::default()
        }
    }
    pub fn observe(&mut self, s: &Snapshot) -> Result<Option<Candle>> {
        ensure!(s.source_fresh, "stale WebSocket packet");
        let ts = i64::from(
            s.exchange_timestamp
                .ok_or_else(|| anyhow::anyhow!("missing exchange timestamp"))?,
        );
        let received = s.received_at_utc.timestamp();
        if let Some(prev) = self.last_exchange_ts {
            ensure!(ts >= prev, "out-of-order exchange timestamp");
        }
        ensure!(
            (-2..=10).contains(&(received - ts)),
            "stale/future exchange timestamp"
        );
        ensure!(
            s.raw.as_ref().is_some_and(|r| r.full.is_some()),
            "not a full WebSocket packet"
        );
        let price: f64 = s.ltp.parse()?;
        ensure!(
            price.is_finite() && price > 0.0,
            "invalid last traded price"
        );
        let bucket = ts.div_euclid(self.step) * self.step;
        ensure!(
            bucket + self.step > self.last_close,
            "late tick for finalized candle"
        );
        if let Some(g) = self.generation {
            ensure!(
                g == s.connection_generation,
                "WebSocket generation changed: candle continuity unverified"
            );
        }
        self.generation = Some(s.connection_generation);
        let cumulative = s
            .cumulative_volume
            .ok_or_else(|| anyhow::anyhow!("missing cumulative volume"))?;
        let delta = match self.last_volume {
            Some(old) if cumulative >= old => u64::from(cumulative - old),
            Some(_) => {
                anyhow::bail!("cumulative volume reset: session/reconnect must be reconciled")
            }
            None => 0,
        };
        self.last_volume = Some(cumulative);
        self.last_exchange_ts = Some(ts);
        let oi = u64::from(s.open_interest.unwrap_or(0));
        if self.current.is_none() {
            // Skip any startup interval that may contain unseen ticks.
            ensure!(bucket >= self.last_close, "WebSocket tick predates warmup");
            // The historical-to-stream gap is never replayed as an entry.
            // The first complete stream bar provides the new live anchor.
            self.skip_partial = ts > bucket;
            // Candles the stream can't supply: the skipped partial one, plus any the
            // warm-up was too early to receive. `live_backfill` fetches them.
            let first_full = if self.skip_partial { bucket + self.step } else { bucket };
            if first_full > self.last_close {
                self.gap = Some((self.last_close, first_full));
            }
            self.current = Some(Current {
                start: bucket,
                open: price,
                high: price,
                low: price,
                close: price,
                volume: 0,
                oi,
            });
        }
        let old_start = self.current.as_ref().expect("current").start;
        ensure!(bucket >= old_start, "out-of-order WebSocket tick");
        if bucket != old_start {
            ensure!(
                bucket == old_start + self.step,
                "WebSocket candle gap after subscription"
            );
            let prev = self.current.take().expect("current");
            self.last_close = prev.start + self.step;
            let skipped = self.skip_partial;
            self.skip_partial = false;
            self.current = Some(Current {
                start: bucket,
                open: price,
                high: price,
                low: price,
                close: price,
                volume: delta,
                oi,
            });
            let timestamp = Utc
                .timestamp_opt(prev.start, 0)
                .single()
                .expect("valid timestamp")
                .to_rfc3339();
            if skipped {
                return Ok(None);
            }
            return Ok(Some(Candle {
                timestamp,
                open: prev.open,
                high: prev.high,
                low: prev.low,
                close: prev.close,
                volume: prev.volume,
                oi: prev.oi,
            }));
        }
        let c = self.current.as_mut().expect("current");
        c.high = c.high.max(price);
        c.low = c.low.min(price);
        c.close = price;
        c.volume += delta;
        c.oi = oi;
        Ok(None)
    }
    /// The startup gap to backfill from history, reported once.
    pub fn take_gap(&mut self) -> Option<(i64, i64)> {
        self.gap.take()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn tick(ts: i64, price: i32, volume: u32, generation: u32) -> Snapshot {
        let mut s = super::super::synthetic::full_snapshot_for(
            145894407,
            price * 100,
            (ts as u64) * 1_000_000_000,
            generation,
        );
        s.received_at_utc = Utc.timestamp_opt(ts, 0).single().unwrap();
        s.exchange_timestamp = Some(ts as u32);
        s.ltp = price.to_string();
        s.cumulative_volume = Some(volume);
        s.connection_generation = generation;
        s.source_fresh = true;
        s
    }
    #[test]
    fn builds_last_price_ohlcv_on_exchange_boundary() {
        let start = 1_800_000_000i64.div_euclid(180) * 180;
        let mut a = Aggregator::new((start as u64) * 1_000_000_000);
        assert!(a.observe(&tick(start, 100, 1000, 1)).unwrap().is_none());
        assert!(
            a.observe(&tick(start + 30, 110, 1007, 1))
                .unwrap()
                .is_none()
        );
        assert!(
            a.observe(&tick(start + 120, 90, 1012, 1))
                .unwrap()
                .is_none()
        );
        let bar = a
            .observe(&tick(start + 180, 105, 1015, 1))
            .unwrap()
            .unwrap();
        assert_eq!(
            (bar.open, bar.high, bar.low, bar.close, bar.volume),
            (100., 110., 90., 90., 12)
        );
    }
    #[test]
    fn five_minute_step_closes_only_on_five_minute_boundaries() {
        let start = 1_800_000_000i64.div_euclid(300) * 300;
        let mut a = Aggregator::with_step((start as u64) * 1_000_000_000, 300);
        assert!(a.observe(&tick(start, 100, 1000, 1)).unwrap().is_none());
        assert!(a.observe(&tick(start + 180, 120, 1010, 1)).unwrap().is_none(), "no 3-minute close");
        let bar = a.observe(&tick(start + 300, 95, 1020, 1)).unwrap().unwrap();
        assert_eq!((bar.open, bar.high, bar.low, bar.close, bar.volume), (100., 120., 100., 120., 10));
        assert_eq!(bar.timestamp, Utc.timestamp_opt(start, 0).single().unwrap().to_rfc3339());
    }
    #[test]
    fn skips_partial_start_and_refuses_gaps_reconnect_and_volume_reset() {
        let start = 1_800_000_000i64.div_euclid(180) * 180;
        let mut partial = Aggregator::new((start as u64) * 1_000_000_000);
        assert!(
            partial
                .observe(&tick(start + 20, 100, 1000, 1))
                .unwrap()
                .is_none()
        );
        assert!(
            partial
                .observe(&tick(start + 180, 101, 1001, 1))
                .unwrap()
                .is_none()
        );
        assert!(
            partial
                .observe(&tick(start + 360, 102, 1002, 1))
                .unwrap()
                .is_some()
        );
        // the skipped partial candle is reported once for backfill
        assert_eq!(partial.take_gap(), Some((start, start + 180)));
        assert_eq!(partial.take_gap(), None);
        let mut gap = Aggregator::new(((start - 360) as u64) * 1_000_000_000);
        assert!(gap.observe(&tick(start, 100, 1000, 1)).unwrap().is_none());
        // first tick on the boundary: no partial, but two candles before it are missing
        assert_eq!(gap.take_gap(), Some((start - 360, start)));
        assert!(
            gap.observe(&tick(start + 180, 101, 1001, 1))
                .unwrap()
                .is_some()
        );
        let mut a = Aggregator::new((start as u64) * 1_000_000_000);
        a.observe(&tick(start, 100, 1000, 1)).unwrap();
        assert!(a.observe(&tick(start + 1, 101, 1001, 2)).is_err());
        let mut b = Aggregator::new((start as u64) * 1_000_000_000);
        b.observe(&tick(start, 100, 1000, 1)).unwrap();
        assert!(b.observe(&tick(start + 180, 101, 900, 1)).is_err());
        let mut c = Aggregator::new((start as u64) * 1_000_000_000);
        c.observe(&tick(start, 100, 1000, 1)).unwrap();
        assert!(c.observe(&tick(start + 360, 101, 1001, 1)).is_err());
        let mut d = Aggregator::new((start as u64) * 1_000_000_000);
        d.observe(&tick(start, 100, 1000, 1)).unwrap();
        d.observe(&tick(start + 60, 110, 1001, 1)).unwrap();
        assert!(d.observe(&tick(start + 59, 109, 1002, 1)).is_err());
    }
}

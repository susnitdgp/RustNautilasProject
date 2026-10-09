//! Startup gap backfill.
//!
//! The candle in progress when the program starts is only partly seen on the
//! WebSocket, so the stream skips it. Without a backfill the strategy never sees
//! that candle at all (nor any finalised candle the warm-up was too early to get).
//! Once Kite's historical API has finalised the gap (≈45 s after it closes) this
//! module fetches exactly those candles so they reach SATS **before** the first
//! live stream bar. If they can't be had in time the gap stays (old behaviour)
//! and the reason is logged; trading never waits on it.
use super::{bar_timing::FINALIZATION_DELAY_NS, live_bars};
use anyhow::{Result, anyhow, ensure};
use chrono::{DateTime, FixedOffset};
use kite_adapter::http::historical::{Candle, Interval, Reader};
use std::sync::{Arc, Mutex};
use tokio::time::Duration;

/// Hand-off between the backfill task and the live stream; whoever locks first
/// decides. The lock is held while bars are sent, so bars stay in time order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Done,
    Abandoned,
}
pub type Shared = Arc<Mutex<State>>;

pub fn shared() -> Shared {
    Arc::new(Mutex::new(State::Pending))
}

const RETRY: Duration = Duration::from_secs(3);
/// Give up this long before the first live bar is due.
const MARGIN_NS: u64 = 5_000_000_000;
const NS: u64 = 1_000_000_000;

fn ist() -> FixedOffset {
    FixedOffset::east_opt(19_800).expect("IST offset")
}

fn ist_date(ns: u64) -> chrono::NaiveDate {
    DateTime::from_timestamp_nanos(ns as i64).with_timezone(&ist()).date_naive()
}

/// Candles whose close lies in `(from_ns, to_ns]`: broker-finalised, in order,
/// contiguous within a day, and ending exactly at `to_ns`.
pub fn select(candles: Vec<Candle>, from_ns: u64, to_ns: u64, now_ns: u64, interval: Interval) -> Result<Vec<Candle>> {
    let finalized = live_bars::broker_finalized_for(candles, now_ns, interval)?;
    let step = interval.nanoseconds();
    let mut out = Vec::new();
    let mut prev = from_ns;
    for c in finalized {
        let close = live_bars::close_for(&c, interval)?;
        if close <= from_ns || close > to_ns {
            continue;
        }
        ensure!(close > prev, "backfill candles out of order");
        if ist_date(close) == ist_date(prev) {
            ensure!(close == prev + step, "gap inside backfill candles");
        }
        prev = close;
        out.push(c);
    }
    ensure!(prev == to_ns, "gap candle not finalised at Kite yet");
    Ok(out)
}

/// Waits for Kite to finalise the gap, then fetches it. `gap` is
/// `(warm-up close, first live bar start)` in epoch seconds.
pub async fn fetch(token: u32, interval: Interval, gap: (i64, i64)) -> Result<Vec<Candle>> {
    let from_ns = u64::try_from(gap.0)? * NS;
    let to_ns = u64::try_from(gap.1)? * NS;
    ensure!(to_ns > from_ns, "empty backfill gap");
    let ready_at = to_ns + FINALIZATION_DELAY_NS + NS;
    let deadline = (to_ns + interval.nanoseconds()).saturating_sub(MARGIN_NS);
    let now = super::data::now();
    if ready_at > now {
        tokio::time::sleep(Duration::from_nanos(ready_at - now)).await;
    }
    let mut reader = Reader::default();
    loop {
        let attempt = async {
            let raw = reader.fetch_window_for(token, ist_date(to_ns), 1, interval).await?;
            select(raw, from_ns, to_ns, super::data::now(), interval)
        }
        .await;
        match attempt {
            Ok(candles) => return Ok(candles),
            Err(e) if super::data::now() + RETRY.as_nanos() as u64 >= deadline => {
                return Err(anyhow!("backfill not ready before the first live bar: {e:#}"));
            }
            Err(_) => tokio::time::sleep(RETRY).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candle(start_ns: u64) -> Candle {
        Candle {
            timestamp: DateTime::from_timestamp_nanos(start_ns as i64).with_timezone(&ist()).to_rfc3339(),
            open: 100.,
            high: 101.,
            low: 99.,
            close: 100.,
            volume: 10,
            oi: 10,
        }
    }
    #[test]
    fn selects_only_the_finalised_gap() {
        let iv = Interval::FiveMinute;
        let step = iv.nanoseconds();
        let base = 1_800_000_000 / 300 * 300 * NS;
        let all: Vec<_> = (0..6).map(|i| candle(base + i * step)).collect();
        // warm-up ended at close of bar 1; first live bar starts at close of bar 3
        let (from, to) = (base + 2 * step, base + 4 * step);
        let late = to + FINALIZATION_DELAY_NS + NS;
        let got = select(all.clone(), from, to, late, iv).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(live_bars::close_for(&got[1], iv).unwrap(), to);
        // too early: the gap's last candle is not finalised yet
        assert!(select(all.clone(), from, to, to + NS, iv).is_err());
        // a hole inside the gap is refused
        let mut holed = all;
        holed.remove(2);
        assert!(select(holed, from, to, late, iv).is_err());
    }
}

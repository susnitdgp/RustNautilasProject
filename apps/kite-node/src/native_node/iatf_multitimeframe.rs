//! September 2026 IATF 1-minute/3-minute candle-only research.
//! No depth, account, order, fills, or live execution.
use anyhow::{Result, ensure};
use chrono::{Datelike, NaiveDate};
use kite_adapter::http::historical::{self, Candle, Interval};
use std::collections::BTreeSet;

fn efficiency(history: &[f64], lookback: usize) -> Option<f64> {
    if history.len() <= lookback {
        return None;
    }
    let x = &history[history.len() - lookback - 1..];
    let total: f64 = x.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
    Some(if total > 0.0 {
        (x[lookback] - x[0]).abs() / total
    } else {
        0.0
    })
}
fn aligned_date(c: &Candle) -> Result<NaiveDate> {
    Ok(c.time()?.date_naive())
}
pub(super) async fn month(token: u32, interval: Interval) -> Result<Vec<Candle>> {
    let mut reader = historical::Reader::default();
    let a = reader
        .fetch_window_for(
            token,
            NaiveDate::from_ymd_opt(2026, 9, 16).unwrap(),
            15,
            interval,
        )
        .await?;
    let b = reader
        .fetch_window_for(
            token,
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            15,
            interval,
        )
        .await?;
    let mut all = a;
    all.extend(b);
    all.sort_by(|x, y| x.timestamp.cmp(&y.timestamp));
    all.dedup_by(|x, y| x.timestamp == y.timestamp);
    all.retain(|c| c.time().is_ok_and(|t| t.year() == 2026 && t.month() == 9));
    historical::validate_for(&all, interval)?;
    Ok(all)
}
#[derive(Default)]
struct Stats {
    candidate: usize,
    forward_count: usize,
    forward_sum: f64,
    positive: usize,
}
fn audit(minute: &[Candle], three: &[Candle]) -> Result<serde_json::Value> {
    ensure!(
        !minute.is_empty() && !three.is_empty(),
        "Missing historical candles"
    );
    let mut dates = BTreeSet::new();
    let mut prev_minute: Vec<f64> = Vec::new();
    let mut three_close: Vec<f64> = Vec::new();
    let mut three_cursor = 0usize;
    let mut last_day = None;
    let mut gated = Stats::default();
    let mut ungated = Stats::default();
    let mut regimes = [0usize; 3];
    let mut missing_minute_intervals = 0usize;
    let mut last_minute_ts = None;
    for (i, c) in minute.iter().enumerate() {
        let date = aligned_date(c)?;
        dates.insert(date);
        let now = c.time()?.timestamp();
        if last_day != Some(date) {
            last_day = Some(date);
            prev_minute.clear();
            three_close.clear();
            last_minute_ts = None;
        }
        if let Some(previous) = last_minute_ts {
            if now != previous + 60 {
                missing_minute_intervals += 1;
                prev_minute.clear();
            }
        }
        last_minute_ts = Some(now);
        // Only completed 3-minute candles whose close <= current 1-minute close.
        while three_cursor < three.len() {
            let t = three[three_cursor].time()?;
            if t.timestamp() + 180 > now + 60 {
                break;
            }
            if t.date_naive() == date {
                three_close.push(three[three_cursor].close);
            }
            three_cursor += 1;
        }
        let er = efficiency(&three_close, 10);
        let regime = match er {
            Some(v) if v < 0.25 => 0,
            Some(v) if v >= 0.45 => 2,
            _ => 1,
        };
        regimes[regime] += 1;
        if prev_minute.len() >= 8 {
            let prev = &prev_minute[prev_minute.len() - 8..];
            let direction = if c.close > prev.iter().copied().fold(f64::NEG_INFINITY, f64::max) {
                1.0
            } else if c.close < prev.iter().copied().fold(f64::INFINITY, f64::min) {
                -1.0
            } else {
                0.0
            };
            if direction != 0.0 {
                ungated.candidate += 1;
                if regime == 2 {
                    gated.candidate += 1;
                }
                // Forward move from next minute OPEN to close of tenth next minute.
                // Descriptive event study ONLY, overlapping observations, no fills.
                if i + 10 < minute.len()
                    && aligned_date(&minute[i + 10])? == date
                    && minute[i + 10].time()?.timestamp() == now + 600
                {
                    let move_points = (minute[i + 10].close - minute[i + 1].open) * direction;
                    ungated.forward_count += 1;
                    ungated.forward_sum += move_points;
                    ungated.positive += usize::from(move_points > 0.0);
                    if regime == 2 {
                        gated.forward_count += 1;
                        gated.forward_sum += move_points;
                        gated.positive += usize::from(move_points > 0.0);
                    }
                }
            }
        }
        prev_minute.push(c.close);
    }
    let summary = |s: &Stats| {
        serde_json::json!({
            "raw_candidate_bars":s.candidate,"forward_10min_events":s.forward_count,
            "positive_forward_events":s.positive,
            "mean_forward_points":if s.forward_count>0{Some(s.forward_sum/s.forward_count as f64)}else{None}
        })
    };
    Ok(
        serde_json::json!({"event":"iatf_september_2026_multitimeframe_candle_study",
            "month":"2026-09","days":dates.len(),"bars_1minute":minute.len(),
            "bars_3minute":three.len(),"gaps_within_sessions":missing_minute_intervals,
            "one_minute_breakouts":summary(&ungated),
            "one_minute_breakouts_with_completed_3min_trend_filter":summary(&gated),
            "three_minute_regime_on_1min_bars":{"chop":regimes[0],"transition_or_warmup":regimes[1],"trend":regimes[2]},
            "trades":null,"net_pnl":null,
            "warning":"Descriptive overlapping candle-only event study; no depth/OFI, fill model, costs, risk, or portfolio accounting."
        }),
    )
}
pub fn run(token: u32) -> Result<()> {
    ensure!(token > 0, "Invalid token");
    let rt = tokio::runtime::Runtime::new()?;
    let (minute, three) = rt.block_on(async {
        Ok::<_, anyhow::Error>((
            month(token, Interval::OneMinute).await?,
            month(token, Interval::ThreeMinute).await?,
        ))
    })?;
    ensure!(minute.len() > 100 && three.len() > 100, "Too little data");
    println!(
        "{}",
        serde_json::to_string_pretty(&audit(&minute, &three)?)?
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn efficiency_classifies_persistent_and_oscillating() {
        let trend = (0..=12).map(|v| v as f64).collect::<Vec<_>>();
        assert_eq!(efficiency(&trend, 10), Some(1.0));
        let chop = (0..=12).map(|v| (v % 2) as f64).collect::<Vec<_>>();
        assert_eq!(efficiency(&chop, 10), Some(0.0));
    }
}

//! Read-only preliminary ILRC gate frequency audit; no broker or strategy changes.
use anyhow::{Result, ensure};
use chrono::{Datelike, NaiveDate};
use kite_adapter::http::historical::{self, Candle, Interval};
use std::collections::BTreeMap;

fn audit(candles: &[Candle]) -> Result<serde_json::Value> {
    historical::validate_for(candles, Interval::ThreeMinute)?;
    let mut counts = BTreeMap::<&'static str, usize>::new();
    let mut day = None;
    let mut same = Vec::<&Candle>::new();
    for c in candles {
        let date = c.time()?.date_naive();
        if day != Some(date) {
            day = Some(date);
            same.clear();
        }
        if same.len() >= 20 {
            *counts.entry("evaluated").or_default() += 1;
            let prev = &same[same.len() - 20..];
            let hi = prev
                .iter()
                .map(|v| v.high)
                .fold(f64::NEG_INFINITY, f64::max);
            let lo = prev.iter().map(|v| v.low).fold(f64::INFINITY, f64::min);
            let long_sweep = c.low < lo && c.close > lo;
            let short_sweep = c.high > hi && c.close < hi;
            if long_sweep || short_sweep {
                *counts.entry("a_sweep_reclaim").or_default() += 1;
            }
            let bos_long = c.close > hi;
            let bos_short = c.close < lo;
            if bos_long || bos_short {
                *counts.entry("b_bos").or_default() += 1;
            }
            let body = (c.close - c.open).abs();
            let avg = prev.iter().map(|v| (v.close - v.open).abs()).sum::<f64>() / 20.0;
            let body_ok = body >= 1.3 * avg;
            if body_ok {
                *counts.entry("b_body_displacement").or_default() += 1;
            }
            let ranges = &same[same.len() - 14..];
            let atr = ranges.iter().map(|v| v.high - v.low).sum::<f64>() / 14.0;
            let atr_ok = atr > 0.0 && body >= 0.8 * atr;
            if atr_ok {
                *counts.entry("b_atr_displacement").or_default() += 1;
            }
            let (mut pv, mut vol) = (0.0, 0.0);
            for bar in same.iter().copied().chain(std::iter::once(c)) {
                let v = bar.volume as f64;
                pv += ((bar.high + bar.low + bar.close) / 3.0) * v;
                vol += v;
            }
            let vwap = if vol > 0.0 { pv / vol } else { c.close };
            if bos_long || bos_short {
                if body_ok && atr_ok {
                    *counts.entry("b_bos_both_displacement").or_default() += 1;
                }
                let aligned = if bos_long {
                    c.close >= vwap
                } else {
                    c.close <= vwap
                };
                if aligned {
                    *counts.entry("b_bos_vwap_aligned").or_default() += 1;
                } else {
                    *counts.entry("b_bos_vwap_rejected").or_default() += 1;
                }
                if body_ok && atr_ok && aligned {
                    *counts.entry("b_preliminary_all_gates").or_default() += 1;
                }
            }
        }
        same.push(c);
    }
    Ok(
        serde_json::json!({"event":"ilrc_preliminary_gate_audit","candles":candles.len(),
        "counts":counts,"limitations":"Preliminary dashboard-style bar gates only. Not full Setup A/B state machine, historical entry eligibility, pullback, stop validity or broker fills."}),
    )
}
pub fn run(token: u32) -> Result<()> {
    ensure!(token > 0, "Invalid instrument token");
    let rt = tokio::runtime::Runtime::new()?;
    let mut candles = rt.block_on(async {
        let mut reader = historical::Reader::default();
        let mut a = reader
            .fetch_window_for(
                token,
                NaiveDate::from_ymd_opt(2026, 9, 16).unwrap(),
                15,
                Interval::ThreeMinute,
            )
            .await?;
        a.extend(
            reader
                .fetch_window_for(
                    token,
                    NaiveDate::from_ymd_opt(2026, 10, 9).unwrap(),
                    23,
                    Interval::ThreeMinute,
                )
                .await?,
        );
        Ok::<_, anyhow::Error>(a)
    })?;
    candles.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    candles.dedup_by(|a, b| a.timestamp == b.timestamp);
    candles.retain(|c| {
        c.time()
            .is_ok_and(|t| t.year() == 2026 && (t.month() == 9 || t.month() == 10 && t.day() <= 8))
    });
    let preliminary = audit(&candles)?;
    let config = super::ilrc_config::Selection::load("config/production-ilrc.json")?;
    ensure!(config.instrument_token == token, "Selection/token mismatch");
    let a = super::ilrc_backtest::entry_events_config_candles(&config, &candles)?;
    let b = super::ilrc_continuation_backtest::entry_events_candles(
        &candles,
        config.continuation.target_r,
    )?;
    let counts = |events: &[super::ilrc_backtest::EntryEvent]| {
        let mut days = std::collections::BTreeMap::<String, usize>::new();
        for event in events {
            *days
                .entry(event.entry_time.chars().take(10).collect())
                .or_default() += 1;
        }
        serde_json::json!({"entry_events":events.len(),"days":days,
            "first_events":events.iter().take(5).map(|e|serde_json::json!({
                "setup":e.setup,"observed_at":e.observed_at,"entry_time":e.entry_time,
                "side":e.side,"entry":e.entry,"stop":e.stop,"target":e.target
            })).collect::<Vec<_>>()})
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "event":"ilrc_full_entry_audit","instrument":config.instrument,
            "candles":candles.len(),"preliminary":preliminary,
            "setup_a":counts(&a),"setup_b":counts(&b),
            "warning":"Actual historical strategy entry-event functions; NOT final broker admissions, next-open fills or per-rejection transition counters."
        }))?
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_candles_rejected() {
        assert!(audit(&[]).is_err());
    }
}

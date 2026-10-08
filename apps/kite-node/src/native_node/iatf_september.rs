//! September 2026 historical candle-only IATF feasibility audit.
//! No depth reconstruction, simulated fills, or claims of strategy profitability.
use anyhow::{Result, ensure};
use chrono::{Datelike, NaiveDate};
use kite_adapter::http::historical::{self, Interval};
use std::collections::BTreeSet;

pub fn run(token: u32) -> Result<()> {
    ensure!(token > 0, "Invalid token");
    let rt = tokio::runtime::Runtime::new()?;
    let mut candles = rt.block_on(async {
        let mut a = historical::fetch_window_for(
            token,
            NaiveDate::from_ymd_opt(2026, 9, 16).unwrap(),
            15,
            Interval::ThreeMinute,
        )
        .await?;
        let b = historical::fetch_window_for(
            token,
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            15,
            Interval::ThreeMinute,
        )
        .await?;
        a.extend(b);
        Ok::<_, anyhow::Error>(a)
    })?;
    candles.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    candles.dedup_by(|a, b| a.timestamp == b.timestamp);
    candles.retain(|c| c.time().is_ok_and(|t| t.year() == 2026 && t.month() == 9));
    historical::validate_for(&candles, Interval::ThreeMinute)?;
    ensure!(
        candles.len() > 100,
        "Insufficient September 3-minute candles"
    );
    let mut dates = BTreeSet::new();
    let mut trend = 0usize;
    let mut chop = 0usize;
    let mut transition = 0usize;
    let mut long_breakout = 0usize;
    let mut short_breakout = 0usize;
    // Signals use prior completed candles, not contemporaneous book imbalance.
    // Per-session restart avoids inventing cross-session continuity.
    let mut history: Vec<f64> = Vec::new();
    let mut last_day = None;
    for c in &candles {
        let day = c.time()?.date_naive();
        dates.insert(day);
        if last_day != Some(day) {
            history.clear();
            last_day = Some(day);
        }
        if history.len() >= 11 {
            let tail = &history[history.len() - 11..];
            let movement: f64 = tail.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
            let er = if movement > 0.0 {
                (tail[10] - tail[0]).abs() / movement
            } else {
                0.0
            };
            if er < 0.25 {
                chop += 1;
            } else if er >= 0.45 {
                trend += 1;
                let pre = &history[history.len() - 8..];
                if c.close > pre.iter().copied().fold(f64::NEG_INFINITY, f64::max) {
                    long_breakout += 1;
                }
                if c.close < pre.iter().copied().fold(f64::INFINITY, f64::min) {
                    short_breakout += 1;
                }
            } else {
                transition += 1;
            }
        }
        history.push(c.close);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "event":"iatf_september_2026_candle_baseline","source":"Kite historical 3minute OHLCV",
            "instrument_token":token,"days":dates.len(),"bars":candles.len(),
            "trend_bars":trend,"chop_bars":chop,"transition_bars":transition,
            "long_breakout_candidates_without_depth":long_breakout,
            "short_breakout_candidates_without_depth":short_breakout,
            "trades":null,"pnl":null,
            "warning":"Not an IATF order-flow backtest. Missing historical order-book depth and execution quotes."
        }))?
    );
    Ok(())
}

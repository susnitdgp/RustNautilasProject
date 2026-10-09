//! Read-only historical replay of one `vce-mojo` portfolio slot. Never places orders.
//!
//! Two fill models are reported side by side:
//! * `pine`      — what the TradingView chart shows: entry at the signal close,
//!   exit at the SL / target level (or the close for EOD), no costs. Use it to
//!   check this port trade-by-trade against the Pine script.
//! * `realistic` — entry at the next bar's open, SL/target filled at the level or
//!   at the open when the bar gaps through it, EOD at the next bar's open, plus
//!   adverse slippage per side and a round-trip cost from the slot config.
use super::{
    backtest_report,
    portfolio::Portfolio,
    vce_config::{self, VceConfig},
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, TimeZone};
use kite_adapter::http::historical::{Candle, Interval, Reader};
use serde::Serialize;
use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf};
use vce_mojo::{BarInput, Event, ExitReason, Levels, Side};

/// Calendar days fetched before `from` to warm the ATRs and the 50-bar median.
const WARMUP_DAYS: i64 = 7;

#[derive(Debug, Serialize)]
pub struct Trade {
    pub side: &'static str,
    pub entry_time: String,
    pub exit_time: String,
    pub reason: &'static str,
    pub bars_held: usize,
    pub coil_bars: usize,
    pub risk_points: f64,
    pub pine_entry: f64,
    pub pine_exit: f64,
    pub pine_points: f64,
    pub fill_entry: f64,
    pub fill_exit: f64,
    pub net_points: f64,
    pub net_rupees: f64,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Stats {
    pub trades: usize,
    pub wins: usize,
    pub win_rate: f64,
    pub points: f64,
    pub profit_factor: Option<f64>,
    pub max_drawdown_points: f64,
    pub best: f64,
    pub worst: f64,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub port_version: &'static str,
    pub instance: String,
    pub instrument: String,
    pub instrument_token: u32,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub bars: usize,
    pub config: VceConfig,
    pub pine: Stats,
    pub realistic: Stats,
    pub realistic_rupees: f64,
    pub realistic_by_month: BTreeMap<String, Stats>,
    pub open_position_at_end: Option<Levels>,
}

pub fn run(portfolio_path: &str, instance_id: &str, from: &str, to: &str) -> Result<()> {
    let portfolio: Portfolio = serde_json::from_str(
        &std::fs::read_to_string(portfolio_path).with_context(|| format!("Cannot read {portfolio_path}"))?,
    )?;
    portfolio.validate()?;
    let inst = portfolio
        .instances
        .iter()
        .find(|v| v.id == instance_id)
        .ok_or_else(|| anyhow::anyhow!("No portfolio instance {instance_id}"))?;
    ensure!(
        inst.strategy == vce_config::STRATEGY,
        "Instance {instance_id} runs \"{}\", not \"{}\"",
        inst.strategy,
        vce_config::STRATEGY
    );
    ensure!(inst.instrument_token != 0, "Instance {instance_id} needs a verified instrument token");
    let config = VceConfig::load(&inst.strategy_config)?;
    let from = NaiveDate::parse_from_str(from, "%Y-%m-%d").context("FROM must be YYYY-MM-DD")?;
    let to = NaiveDate::parse_from_str(to, "%Y-%m-%d").context("TO must be YYYY-MM-DD")?;
    ensure!(from <= to, "FROM must not be after TO");

    let candles = fetch(inst.instrument_token, from - Duration::days(WARMUP_DAYS), to, config.interval())?;
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);
    let bars = candles
        .iter()
        .map(|c| config.bar(c))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|b| b.close_time_ns <= now_ns) // never score a still-forming candle
        .collect::<Vec<_>>();
    ensure!(!bars.is_empty(), "No completed candles in range");

    let ist = ist(&config);
    let from_ns = ist
        .from_local_datetime(&from.and_hms_opt(0, 0, 0).expect("midnight"))
        .single()
        .and_then(|d| d.timestamp_nanos_opt())
        .ok_or_else(|| anyhow::anyhow!("Invalid FROM"))?;
    let (trades, open) = replay(&bars, &config, from_ns)?;
    let pine = stats(trades.iter().map(|t| t.pine_points));
    let realistic = stats(trades.iter().map(|t| t.net_points));
    let mut months: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for t in &trades {
        months.entry(t.entry_time[..7].to_owned()).or_default().push(t.net_points);
    }
    let report = Report {
        port_version: vce_mojo::PORT_VERSION,
        instance: inst.id.clone(),
        instrument: inst.instrument.clone(),
        instrument_token: inst.instrument_token,
        from,
        to,
        bars: bars.len(),
        realistic_rupees: round2(realistic.points * config.point_value * f64::from(config.lots)),
        config,
        pine,
        realistic,
        realistic_by_month: months.into_iter().map(|(k, v)| (k, stats(v.into_iter()))).collect(),
        open_position_at_end: open,
    };

    let dir = PathBuf::from("backtest_results")
        .join(vce_config::STRATEGY)
        .join(&inst.id)
        .join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
    std::fs::create_dir_all(&dir)?;
    backtest_report::json(&dir, "summary.json", &report)?;
    std::fs::write(dir.join("trades.csv"), csv(&trades))?;
    println!(
        "{}",
        serde_json::json!({
            "event": "vce_backtest",
            "instance": report.instance,
            "from": report.from, "to": report.to, "bars": report.bars,
            "pine": report.pine, "realistic": report.realistic,
            "realistic_rupees": report.realistic_rupees,
            "output": dir,
        })
    );
    Ok(())
}

fn ist(config: &VceConfig) -> FixedOffset {
    FixedOffset::east_opt(config.params.utc_offset_minutes * 60).expect("validated offset")
}

fn fetch(token: u32, first: NaiveDate, last: NaiveDate, interval: Interval) -> Result<Vec<Candle>> {
    let rt = tokio::runtime::Runtime::new()?;
    let mut candles = rt.block_on(async {
        let mut reader = Reader::default();
        let mut all = Vec::new();
        let mut start = first;
        while start <= last {
            let end = (start + Duration::days(25)).min(last);
            // At least 5 days per request so a weekend-only window is never empty.
            let days = (end - start).num_days().max(5);
            all.extend(reader.fetch_window_for(token, end, days, interval).await?);
            start = end + Duration::days(1);
        }
        Ok::<_, anyhow::Error>(all)
    })?;
    candles.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    candles.dedup_by(|a, b| a.timestamp == b.timestamp);
    Ok(candles)
}

/// Runs the engine over `bars`; scores trades entered at or after `from_ns`.
pub fn replay(bars: &[BarInput], config: &VceConfig, from_ns: i64) -> Result<(Vec<Trade>, Option<Levels>)> {
    let mut engine = config.engine()?;
    let mut open: Option<(usize, Levels)> = None;
    let mut trades = Vec::new();
    for (i, bar) in bars.iter().enumerate() {
        for event in engine.on_bar(bar) {
            match event {
                Event::Entry { levels, .. } => open = Some((i, levels)),
                Event::Exit { reason, price, .. } => {
                    if let Some((entry_idx, levels)) = open.take()
                        && levels.entry_time_ns >= from_ns
                    {
                        trades.push(trade(bars, entry_idx, i, &levels, reason, price, config));
                    }
                }
            }
        }
    }
    Ok((trades, open.map(|(_, l)| l).filter(|l| l.entry_time_ns >= from_ns)))
}

fn trade(
    bars: &[BarInput],
    entry_idx: usize,
    exit_idx: usize,
    t: &Levels,
    reason: ExitReason,
    pine_exit: f64,
    config: &VceConfig,
) -> Trade {
    let dir = if t.side == Side::Long { 1.0 } else { -1.0 };
    let slip = config.costs.slippage_points_per_side;
    let exit_bar = &bars[exit_idx];
    let next_open = |i: usize| bars.get(i + 1).map(|b| b.open);

    let fill_entry = next_open(entry_idx).unwrap_or(t.entry) + dir * slip;
    let base_exit = match reason {
        // A bar that opens beyond the level fills at the open, not the level.
        ExitReason::StopLoss => {
            if dir * (exit_bar.open - t.sl) <= 0.0 { exit_bar.open } else { t.sl }
        }
        ExitReason::Target(_) => {
            let tgt = t.target_price();
            if dir * (exit_bar.open - tgt) >= 0.0 { exit_bar.open } else { tgt }
        }
        // The EOD alert fires at the cut-off bar's close; the order fills next.
        ExitReason::EndOfDay => next_open(exit_idx)
            .filter(|_| same_day(exit_bar, bars.get(exit_idx + 1), config))
            .unwrap_or(exit_bar.close),
    };
    let fill_exit = base_exit - dir * slip;
    let net_points = dir * (fill_exit - fill_entry) - config.costs.round_trip_points;
    let fmt = |ns: i64| DateTime::from_timestamp_nanos(ns).with_timezone(&ist(config)).format("%Y-%m-%d %H:%M").to_string();
    Trade {
        side: if t.side == Side::Long { "LONG" } else { "SHORT" },
        entry_time: fmt(t.entry_time_ns),
        exit_time: fmt(exit_bar.close_time_ns),
        reason: reason.label(),
        bars_held: exit_idx - entry_idx,
        coil_bars: t.coil_bars,
        risk_points: round2(t.risk),
        pine_entry: round2(t.entry),
        pine_exit: round2(pine_exit),
        pine_points: round2(dir * (pine_exit - t.entry)),
        fill_entry: round2(fill_entry),
        fill_exit: round2(fill_exit),
        net_points: round2(net_points),
        net_rupees: round2(net_points * config.point_value * f64::from(config.lots)),
    }
}

fn same_day(a: &BarInput, b: Option<&BarInput>, config: &VceConfig) -> bool {
    let day = |ns: i64| DateTime::from_timestamp_nanos(ns).with_timezone(&ist(config)).date_naive();
    b.is_some_and(|b| day(a.open_time_ns) == day(b.open_time_ns))
}

pub fn stats(points: impl Iterator<Item = f64>) -> Stats {
    let pts: Vec<f64> = points.collect();
    if pts.is_empty() {
        return Stats::default();
    }
    let wins = pts.iter().filter(|p| **p > 0.0).count();
    let profit: f64 = pts.iter().filter(|p| **p > 0.0).sum();
    let loss: f64 = -pts.iter().filter(|p| **p < 0.0).sum::<f64>();
    let (mut equity, mut peak, mut dd) = (0.0_f64, 0.0_f64, 0.0_f64);
    for p in &pts {
        equity += p;
        peak = peak.max(equity);
        dd = dd.max(peak - equity);
    }
    Stats {
        trades: pts.len(),
        wins,
        win_rate: round2(100.0 * wins as f64 / pts.len() as f64),
        points: round2(pts.iter().sum()),
        profit_factor: (loss > 0.0).then(|| round2(profit / loss)),
        max_drawdown_points: round2(dd),
        best: round2(pts.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        worst: round2(pts.iter().copied().fold(f64::INFINITY, f64::min)),
    }
}

fn csv(trades: &[Trade]) -> String {
    let mut out = String::from(
        "side,entry_time,exit_time,reason,bars_held,coil_bars,risk_points,pine_entry,pine_exit,pine_points,fill_entry,fill_exit,net_points,net_rupees\n",
    );
    for t in trades {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            t.side, t.entry_time, t.exit_time, t.reason, t.bars_held, t.coil_bars, t.risk_points,
            t.pine_entry, t.pine_exit, t.pine_points, t.fill_entry, t.fill_exit, t.net_points, t.net_rupees
        );
    }
    out
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use vce_mojo::ExitTarget;

    fn cfg() -> VceConfig {
        serde_json::from_str(include_str!("../../../../config/vce-mojo-crudeoilm.json")).unwrap()
    }
    fn bar(i: i64, o: f64, h: f64, l: f64, c: f64) -> BarInput {
        let t = 1_790_000_000_000_000_000 + i * 60_000_000_000;
        BarInput { open_time_ns: t, close_time_ns: t + 60_000_000_000, open: o, high: h, low: l, close: c }
    }
    fn short_levels() -> Levels {
        Levels {
            side: Side::Short, entry: 100.0, sl: 110.0, tp1: 90.0, tp2: 85.0, tp3: 80.0, risk: 10.0,
            exit_target: ExitTarget::Tp1, entry_bar: 0, entry_time_ns: 0, coil_bars: 5, tp1_hit: false, tp2_hit: false,
        }
    }

    #[test]
    fn realistic_fills_use_next_open_gaps_slippage_and_costs() {
        let bars = [bar(0, 100.0, 101.0, 99.0, 100.0), bar(1, 99.0, 100.0, 85.0, 86.0)];
        let t = trade(&bars, 0, 1, &short_levels(), ExitReason::Target(ExitTarget::Tp1), 90.0, &cfg());
        assert_eq!(t.pine_points, 10.0);
        assert_eq!(t.fill_entry, 98.5); // next open 99 minus 0.5 adverse for a short
        assert_eq!(t.fill_exit, 90.5); // TP1 90 plus 0.5 adverse
        assert_eq!(t.net_points, 2.0); // 8 - 6 round trip (CRUDEOILM charges in points)
        assert_eq!(t.net_rupees, 20.0); // ₹10 per point

        // Bar gaps straight through the stop: filled at the open, not the level.
        let gap = [bar(0, 100.0, 101.0, 99.0, 100.0), bar(1, 115.0, 116.0, 114.0, 115.0)];
        let t = trade(&gap, 0, 1, &short_levels(), ExitReason::StopLoss, 110.0, &cfg());
        assert_eq!(t.fill_exit, 115.5);
        assert_eq!(t.pine_points, -10.0);
    }

    #[test]
    fn stats_profit_factor_and_drawdown() {
        let s = stats([10.0, -5.0, -5.0, 20.0].into_iter());
        assert_eq!((s.trades, s.wins, s.points), (4, 2, 20.0));
        assert_eq!(s.profit_factor, Some(3.0));
        assert_eq!(s.max_drawdown_points, 10.0);
        assert_eq!(stats(std::iter::empty()), Stats::default());
    }
}

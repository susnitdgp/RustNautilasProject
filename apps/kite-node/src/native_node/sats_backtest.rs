//! Read-only historical replay of one `sats` portfolio slot. Never places orders.
//!
//! Two views per trade:
//! * `model`     — the SATS script's own accounting (TradingView trader card):
//!   entry at the signal close, ⅓ partials at TP1/TP2 limits, rest at TP3, SL
//!   with gap fill, flip/timeout at the close; net R and points per 1 unit.
//! * `execution` — what the slot's `execution` settings would actually do with
//!   `lots`: each event is a market order filled at the next bar's open with
//!   adverse slippage, plus the round-trip cost per lot; in points and rupees.
use super::{
    backtest_report,
    portfolio::Portfolio,
    sats_config::{self, SatsConfig},
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, TimeZone};
use kite_adapter::http::historical::{Candle, Interval, Reader};
use sats::{BarInput, Event, Side};
use serde::Serialize;
use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf};

/// Calendar days fetched before FROM for the ATR baseline / RSI memory warm-up.
const WARMUP_DAYS: i64 = 10;

fn ist() -> FixedOffset {
    FixedOffset::east_opt(19_800).expect("IST")
}

#[derive(Debug, Serialize)]
pub struct Trade {
    pub side: &'static str,
    pub entry_time: String,
    pub exit_time: String,
    pub exit_reason: &'static str,
    pub entry_reason: String,
    pub score: f64,
    pub tqi: f64,
    pub bars_held: i64,
    pub model_entry: f64,
    pub sl: f64,
    pub tp1: f64,
    pub tp2: f64,
    pub tp3: f64,
    pub risk_points: f64,
    pub model_net_r: f64,
    pub model_points: f64,
    pub exec_entry: f64,
    pub exec_exit_avg: f64,
    pub exec_points: f64,
    pub exec_rupees: f64,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Stats {
    pub trades: usize,
    pub wins: usize,
    pub win_rate: f64,
    pub total: f64,
    pub average: f64,
    pub profit_factor: Option<f64>,
    pub max_drawdown: f64,
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
    pub config: SatsConfig,
    pub resolved_preset: String,
    pub model_r: Stats,
    pub model_points: Stats,
    pub execution_points: Stats,
    pub execution_rupees: f64,
    pub execution_by_month: BTreeMap<String, Stats>,
    pub open_trade_at_end: Option<sats::TradeSnapshot>,
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
        inst.strategy == sats_config::STRATEGY,
        "Instance {instance_id} runs \"{}\", not \"{}\"",
        inst.strategy,
        sats_config::STRATEGY
    );
    ensure!(inst.instrument_token != 0, "Instance {instance_id} needs a verified instrument token");
    let config = SatsConfig::load(&inst.strategy_config)?;
    let from = NaiveDate::parse_from_str(from, "%Y-%m-%d").context("FROM must be YYYY-MM-DD")?;
    let to = NaiveDate::parse_from_str(to, "%Y-%m-%d").context("TO must be YYYY-MM-DD")?;
    ensure!(from <= to, "FROM must not be after TO");

    let candles = fetch(inst.instrument_token, from - Duration::days(WARMUP_DAYS), to, config.interval()?)?;
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);
    let bars = candles
        .iter()
        .map(|c| config.bar(c))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|b| b.close_time_ns <= now_ns) // never score a still-forming candle
        .collect::<Vec<_>>();
    ensure!(!bars.is_empty(), "No completed candles in range");
    let from_ns = ist()
        .from_local_datetime(&from.and_hms_opt(0, 0, 0).expect("midnight"))
        .single()
        .and_then(|d| d.timestamp_nanos_opt())
        .ok_or_else(|| anyhow::anyhow!("Invalid FROM"))?;

    let (trades, open, preset) = replay(&bars, &config, from_ns)?;
    let mut months: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for t in &trades {
        months.entry(t.entry_time[..7].to_owned()).or_default().push(t.exec_points);
    }
    let execution_points = stats(trades.iter().map(|t| t.exec_points));
    let report = Report {
        port_version: sats::PORT_VERSION,
        instance: inst.id.clone(),
        instrument: inst.instrument.clone(),
        instrument_token: inst.instrument_token,
        from,
        to,
        bars: bars.len(),
        resolved_preset: preset,
        model_r: stats(trades.iter().map(|t| t.model_net_r)),
        model_points: stats(trades.iter().map(|t| t.model_points)),
        execution_rupees: round2(execution_points.total * config.point_value),
        execution_points,
        execution_by_month: months.into_iter().map(|(k, v)| (k, stats(v.into_iter()))).collect(),
        open_trade_at_end: open,
        config,
    };

    let dir = PathBuf::from("backtest_results")
        .join(sats_config::STRATEGY)
        .join(&inst.id)
        .join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
    std::fs::create_dir_all(&dir)?;
    backtest_report::json(&dir, "summary.json", &report)?;
    std::fs::write(dir.join("trades.csv"), csv(&trades))?;
    println!(
        "{}",
        serde_json::json!({
            "event": "sats_backtest",
            "instance": report.instance,
            "from": report.from, "to": report.to, "bars": report.bars,
            "preset": report.resolved_preset,
            "model_r": report.model_r,
            "model_points": report.model_points,
            "execution_points": report.execution_points,
            "execution_rupees": report.execution_rupees,
            "output": dir,
        })
    );
    Ok(())
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

/// One model trade's events, in order.
struct Open {
    entry: Event,
    entry_idx: usize,
    exec_entry: f64,
    open_lots: u32,
    exit_lots: u32,
    exit_value: f64,
}

/// Runs the engine over `bars` and scores trades entered at or after `from_ns`.
pub fn replay(
    bars: &[BarInput],
    config: &SatsConfig,
    from_ns: i64,
) -> Result<(Vec<Trade>, Option<sats::TradeSnapshot>, String)> {
    let mut engine = config.engine()?;
    let preset = format!("{:?}", engine.resolved().preset);
    let slip = config.execution.slippage_points_per_side;
    let lots = config.lots;
    let next_open = |i: usize| bars.get(i + 1).map_or(bars[i].close, |b| b.open);
    let mut open: Option<Open> = None;
    let mut trades = Vec::new();
    for (i, bar) in bars.iter().enumerate() {
        for ev in engine.on_bar(bar) {
            let d = ev.trade.side.sign();
            if ev.kind.is_entry() {
                open = Some(Open {
                    exec_entry: next_open(i) + d * slip,
                    entry: ev,
                    entry_idx: i,
                    open_lots: lots,
                    exit_lots: 0,
                    exit_value: 0.0,
                });
                continue;
            }
            let Some(o) = open.as_mut() else { continue };
            let n = config.execution.lots_to_close(ev.kind, lots, o.open_lots);
            if n > 0 {
                o.exit_value += f64::from(n) * (next_open(i) - d * slip);
                o.exit_lots += n;
                o.open_lots -= n;
            }
            if ev.closes_trade {
                let o = open.take().expect("open trade");
                if o.entry.trade.entry_time_ns >= from_ns {
                    trades.push(finish(&o, &ev, i, config));
                }
            }
        }
    }
    let still_open = open.map(|o| o.entry.trade).filter(|t| t.entry_time_ns >= from_ns);
    Ok((trades, still_open, preset))
}

fn finish(o: &Open, last: &Event, exit_idx: usize, config: &SatsConfig) -> Trade {
    let t = &last.trade;
    let d = t.side.sign();
    let exit_avg = if o.exit_lots > 0 { o.exit_value / f64::from(o.exit_lots) } else { o.exec_entry };
    let lots = f64::from(o.exit_lots.max(1));
    // Points for the whole position (all lots), net of the per-lot round-trip cost.
    let exec_points = lots * (d * (exit_avg - o.exec_entry) - config.execution.round_trip_cost_points);
    let fmt = |ns: i64| DateTime::from_timestamp_nanos(ns).with_timezone(&ist()).format("%Y-%m-%d %H:%M").to_string();
    Trade {
        side: if t.side == Side::Long { "LONG" } else { "SHORT" },
        entry_time: fmt(t.entry_time_ns),
        exit_time: fmt(last.time_ns),
        exit_reason: last.kind.label(),
        entry_reason: t.reason.clone(),
        score: round2(t.entry_score),
        tqi: round2(t.entry_tqi),
        bars_held: (exit_idx - o.entry_idx) as i64,
        model_entry: t.entry,
        sl: t.sl,
        tp1: t.tp1,
        tp2: t.tp2,
        tp3: t.tp3,
        risk_points: round2(t.risk),
        model_net_r: round2(t.net_r()),
        model_points: round2(t.net_r() * t.risk),
        exec_entry: round2(o.exec_entry),
        exec_exit_avg: round2(exit_avg),
        exec_points: round2(exec_points),
        exec_rupees: round2(exec_points * config.point_value),
    }
}

pub fn stats(values: impl Iterator<Item = f64>) -> Stats {
    let v: Vec<f64> = values.collect();
    if v.is_empty() {
        return Stats::default();
    }
    let wins = v.iter().filter(|x| **x > 0.0).count();
    let profit: f64 = v.iter().filter(|x| **x > 0.0).sum();
    let loss: f64 = -v.iter().filter(|x| **x < 0.0).sum::<f64>();
    let (mut eq, mut peak, mut dd) = (0.0_f64, 0.0_f64, 0.0_f64);
    for x in &v {
        eq += x;
        peak = peak.max(eq);
        dd = dd.max(peak - eq);
    }
    let total: f64 = v.iter().sum();
    Stats {
        trades: v.len(),
        wins,
        win_rate: round2(100.0 * wins as f64 / v.len() as f64),
        total: round2(total),
        average: round2(total / v.len() as f64),
        profit_factor: (loss > 0.0).then(|| round2(profit / loss)),
        max_drawdown: round2(dd),
        best: round2(v.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        worst: round2(v.iter().copied().fold(f64::INFINITY, f64::min)),
    }
}

fn csv(trades: &[Trade]) -> String {
    let mut out = String::from(
        "side,entry_time,exit_time,exit_reason,entry_reason,score,tqi,bars_held,model_entry,sl,tp1,tp2,tp3,risk_points,model_net_r,model_points,exec_entry,exec_exit_avg,exec_points,exec_rupees\n",
    );
    for t in trades {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            t.side, t.entry_time, t.exit_time, t.exit_reason, t.entry_reason, t.score, t.tqi, t.bars_held,
            t.model_entry, t.sl, t.tp1, t.tp2, t.tp3, t.risk_points, t.model_net_r, t.model_points,
            t.exec_entry, t.exec_exit_avg, t.exec_points, t.exec_rupees
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

    #[test]
    fn stats_profit_factor_and_drawdown() {
        let s = stats([10.0, -5.0, -5.0, 20.0].into_iter());
        assert_eq!((s.trades, s.wins, s.total), (4, 2, 20.0));
        assert_eq!(s.profit_factor, Some(3.0));
        assert_eq!(s.max_drawdown, 10.0);
        assert_eq!(stats(std::iter::empty()), Stats::default());
    }

    #[test]
    fn replay_runs_end_to_end_on_synthetic_bars() {
        let config: SatsConfig = serde_json::from_str(include_str!("../../../../config/sats-crudeoilm.json")).unwrap();
        let mut price = 5000.0_f64;
        let bars: Vec<BarInput> = (0..600)
            .map(|i| {
                let drift = if i < 250 { 3.0 } else if i < 400 { -6.0 } else { 4.0 };
                let open = price;
                price += drift + ((i as f64) * 0.7).sin() * 4.0;
                let t = 1_790_000_000_000_000_000_i64 + i as i64 * 300_000_000_000;
                BarInput {
                    open_time_ns: t,
                    close_time_ns: t + 300_000_000_000,
                    open,
                    high: open.max(price) + 2.0,
                    low: open.min(price) - 2.0,
                    close: price,
                    volume: Some(1000.0),
                }
            })
            .collect();
        let (trades, _, preset) = replay(&bars, &config, 0).unwrap();
        assert_eq!(preset, "Scalping");
        assert!(!trades.is_empty());
        for t in &trades {
            // single exit: the whole lot leaves at one price; costs are charged once per lot
            assert!(t.exec_points.is_finite() && t.model_net_r.is_finite());
            assert!(["TP2", "TP3", "SL", "FLIP", "TIMEOUT"].contains(&t.exit_reason) || t.exit_reason == "TP1");
        }
    }
}

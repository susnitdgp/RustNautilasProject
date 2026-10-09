//! Read-only CRUDE-BURST replay on Kite 1-minute history. Never places orders.
//!
//! Each bar: (1) a signal from the previous bar's close enters at this bar's open
//! with adverse slippage; (2) stop/target are checked on this bar's OHLC — a gap
//! fills at the open, and when both are touched the stop is taken first;
//! (3) the engine sees the closed bar; (4) breakeven / trail / time stop and the
//! daily square-off act at the close; (5) a new signal is queued if flat.
//! Every exit pays slippage; every trade pays `round_trip_cost_points`.
use super::{
    backtest_report,
    burst_config::{BurstConfig, STRATEGY},
};
use anyhow::{Result, ensure};
use burst::{Bar, DayRisk, Engine, ExitReason, Signal, Trade};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate};
use kite_adapter::http::historical::{Candle, Interval, Reader};
use serde::Serialize;
use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf};

/// Calendar days fetched before FROM to warm ATR / volume / previous-day levels.
pub const WARMUP_DAYS: i64 = 4;

fn ist() -> FixedOffset {
    FixedOffset::east_opt(19_800).expect("IST")
}
fn ist_time(ts: i64) -> DateTime<FixedOffset> {
    DateTime::from_timestamp(ts, 0).expect("timestamp").with_timezone(&ist())
}
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[derive(Debug, Clone, Serialize)]
pub struct TradeRow {
    pub date: String,
    pub side: &'static str,
    pub signal_time: String,
    pub entry_time: String,
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
    pub risk_points: f64,
    pub exit_time: String,
    pub exit_reason: &'static str,
    pub exit_price: f64,
    pub bars_held: u32,
    pub max_r: f64,
    pub gross_points: f64,
    pub net_points: f64,
    pub net_rupees: f64,
    pub volume_ratio: f64,
    pub atr: f64,
    pub reason: String,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
pub struct Stats {
    pub trades: usize,
    pub wins: usize,
    pub win_rate: f64,
    pub net_points: f64,
    pub net_rupees: f64,
    pub avg_win_points: f64,
    pub avg_loss_points: f64,
    pub profit_factor: f64,
    pub expectancy_points: f64,
    pub max_drawdown_rupees: f64,
}

pub fn stats(rows: &[&TradeRow]) -> Stats {
    let n = rows.len();
    if n == 0 {
        return Stats::default();
    }
    let wins: Vec<f64> = rows.iter().map(|r| r.net_points).filter(|p| *p > 0.0).collect();
    let losses: Vec<f64> = rows.iter().map(|r| r.net_points).filter(|p| *p <= 0.0).collect();
    // fold from +0.0: an empty f64 sum is -0.0, which prints as "-0.00"
    let gross_win: f64 = wins.iter().fold(0.0, |a, b| a + b);
    let gross_loss: f64 = losses.iter().fold(0.0, |a, b| a - b);
    let net_points: f64 = rows.iter().fold(0.0, |a, r| a + r.net_points);
    let (mut equity, mut peak, mut dd) = (0.0_f64, 0.0_f64, 0.0_f64);
    for r in rows {
        equity += r.net_rupees;
        peak = peak.max(equity);
        dd = dd.max(peak - equity);
    }
    Stats {
        trades: n,
        wins: wins.len(),
        win_rate: round2(wins.len() as f64 / n as f64 * 100.0),
        net_points: round2(net_points),
        net_rupees: round2(rows.iter().fold(0.0, |a, r| a + r.net_rupees)),
        avg_win_points: round2(if wins.is_empty() { 0.0 } else { gross_win / wins.len() as f64 }),
        avg_loss_points: round2(if losses.is_empty() { 0.0 } else { -gross_loss / losses.len() as f64 }),
        profit_factor: round2(if gross_loss > 0.0 { gross_win / gross_loss } else { f64::INFINITY.min(999.0) }),
        expectancy_points: round2(net_points / n as f64),
        max_drawdown_rupees: round2(dd),
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub strategy: &'static str,
    pub symbol: String,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub bars: usize,
    pub trading_days: usize,
    pub signals: usize,
    pub skipped_signals: BTreeMap<String, usize>,
    /// Bars that were not signals, by the first rule that failed (the tuning funnel).
    pub rejected_bars: BTreeMap<String, u64>,
    pub overall: Stats,
    pub by_side: BTreeMap<String, Stats>,
    pub by_hour_ist: BTreeMap<String, Stats>,
    pub by_month: BTreeMap<String, Stats>,
    pub by_exit: BTreeMap<String, Stats>,
    /// First vs second half of the trading days: does the edge hold over time?
    pub first_half: Stats,
    pub second_half: Stats,
    pub config: BurstConfig,
}

struct Open {
    trade: Trade,
    signal: Signal,
    entry_ts: i64,
}

pub fn bars_from(candles: &[Candle]) -> Result<Vec<Bar>> {
    candles
        .iter()
        .map(|c| {
            Ok(Bar {
                start: c.time()?.timestamp(),
                seconds: 60,
                open: c.open,
                high: c.high,
                low: c.low,
                close: c.close,
                volume: c.volume as f64,
            })
        })
        .collect()
}

/// Replays `bars` (oldest first). Bars before `from` only warm the engine.
pub type Replay = (Vec<TradeRow>, usize, BTreeMap<String, usize>, BTreeMap<String, u64>);

pub fn replay(config: &BurstConfig, bars: &[Bar], from: NaiveDate) -> Replay {
    let p = &config.params;
    let slip = config.slippage_points_per_side;
    let mut engine = Engine::new(p.clone());
    let mut risk = DayRisk::default();
    let mut open: Option<Open> = None;
    let mut pending: Option<Signal> = None;
    let mut rows = Vec::new();
    let mut signals = 0;
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut skip = |why: &str| *skipped.entry(why.to_owned()).or_default() += 1;

    let close_trade = |o: Open, ts: i64, reason: ExitReason, px: f64, rows: &mut Vec<TradeRow>, risk: &mut DayRisk| {
        let d = o.trade.side.dir();
        let fill = px - d * slip;
        let gross = (fill - o.trade.entry) * d;
        let net = gross - config.round_trip_cost_points;
        let date = ist_time(o.entry_ts).date_naive();
        risk.on_exit(date, net);
        rows.push(TradeRow {
            date: date.to_string(),
            side: o.trade.side.label(),
            signal_time: ist_time(o.signal.bar_close - 60).format("%H:%M").to_string(),
            entry_time: ist_time(o.entry_ts).format("%H:%M").to_string(),
            entry: round2(o.trade.entry),
            stop: round2(o.signal.stop),
            target: round2(o.trade.target),
            risk_points: round2(o.trade.risk),
            exit_time: ist_time(ts).format("%H:%M").to_string(),
            exit_reason: reason.label(),
            exit_price: round2(fill),
            bars_held: o.trade.bars_held,
            max_r: round2(o.trade.max_r),
            gross_points: round2(gross),
            net_points: round2(net),
            net_rupees: round2(net * config.point_value * config.lots as f64),
            volume_ratio: round2(o.signal.volume_ratio),
            atr: round2(o.signal.atr),
            reason: o.signal.reason.clone(),
        });
    };

    for (i, bar) in bars.iter().enumerate() {
        let trading = bar.session_date() >= from;
        let close_t = bar.close_time_ist();
        // (1) queued entry at this bar's open
        if let Some(sig) = pending.take()
            && open.is_none()
        {
            if bar.start != sig.bar_close {
                skip("next bar missing");
            } else if close_t > config.square_off {
                skip("after square-off");
            } else if let Err(why) = risk.allowed(bar.session_date(), p) {
                skip(why);
            } else {
                let fill = bar.open + sig.side.dir() * slip;
                match Trade::open(sig.side, fill, sig.stop, p) {
                    Some(trade) => {
                        risk.on_entry(bar.session_date());
                        open = Some(Open { trade, signal: sig, entry_ts: bar.start });
                    }
                    None => skip("gapped through stop / too wide at fill"),
                }
            }
        }
        // (2) intrabar stop / target
        if let Some(o) = &open
            && let Some((reason, px)) = o.trade.on_bar(bar.open, bar.high, bar.low)
        {
            let o = open.take().expect("open");
            close_trade(o, bar.start, reason, px, &mut rows, &mut risk);
        }
        // (3) engine sees the closed bar
        let signal = engine.on_bar(*bar);
        // (4) bar-close management
        let last_of_day = bars.get(i + 1).is_none_or(|n| n.session_date() != bar.session_date());
        if let Some(o) = &mut open {
            let swing = engine.swing(o.trade.side, p.trail_swing_bars);
            let reason = o
                .trade
                .on_bar_close(bar.high, bar.low, bar.close, swing, p)
                .or((close_t >= config.square_off || last_of_day).then_some(ExitReason::SquareOff));
            if let Some(reason) = reason {
                let o = open.take().expect("open");
                close_trade(o, bar.close_time(), reason, bar.close, &mut rows, &mut risk);
            }
        }
        // (5) queue a new entry
        if let Some(sig) = signal
            && trading
        {
            signals += 1;
            if open.is_some() {
                skip("already in a trade");
            } else if close_t >= config.square_off || last_of_day {
                skip("after square-off");
            } else {
                pending = Some(sig);
            }
        }
    }
    let rejects = engine.rejects().iter().map(|(k, v)| ((*k).to_owned(), *v)).collect();
    (rows, signals, skipped, rejects)
}

pub fn fetch(token: u32, first: NaiveDate, last: NaiveDate) -> Result<Vec<Candle>> {
    let rt = tokio::runtime::Runtime::new()?;
    let mut candles = rt.block_on(async {
        let mut reader = Reader::default();
        let mut all = Vec::new();
        let mut start = first;
        while start <= last {
            let end = (start + Duration::days(25)).min(last);
            let days = (end - start).num_days().max(5);
            all.extend(reader.fetch_window_for(token, end, days, Interval::OneMinute).await?);
            start = end + Duration::days(1);
        }
        Ok::<_, anyhow::Error>(all)
    })?;
    candles.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    candles.dedup_by(|a, b| a.timestamp == b.timestamp);
    Ok(candles)
}

fn group<'a>(rows: &'a [TradeRow], key: impl Fn(&TradeRow) -> String) -> BTreeMap<String, Stats> {
    let mut map: BTreeMap<String, Vec<&'a TradeRow>> = BTreeMap::new();
    for r in rows {
        map.entry(key(r)).or_default().push(r);
    }
    map.into_iter().map(|(k, v)| (k, stats(&v))).collect()
}

pub fn report(config: &BurstConfig, bars: &[Bar], from: NaiveDate, to: NaiveDate) -> (Report, Vec<TradeRow>) {
    let (rows, signals, skipped, rejects) = replay(config, bars, from);
    let mut days: Vec<NaiveDate> = bars.iter().map(|b| b.session_date()).filter(|d| *d >= from && *d <= to).collect();
    days.dedup();
    let mid = days.get(days.len() / 2).copied().unwrap_or(from).to_string();
    let all: Vec<&TradeRow> = rows.iter().collect();
    let first: Vec<&TradeRow> = rows.iter().filter(|r| r.date < mid).collect();
    let second: Vec<&TradeRow> = rows.iter().filter(|r| r.date >= mid).collect();
    let report = Report {
        strategy: STRATEGY,
        symbol: config.symbol.clone(),
        from,
        to,
        bars: bars.iter().filter(|b| b.session_date() >= from).count(),
        trading_days: days.len(),
        signals,
        skipped_signals: skipped,
        rejected_bars: rejects,
        overall: stats(&all),
        by_side: group(&rows, |r| r.side.to_owned()),
        by_hour_ist: group(&rows, |r| r.entry_time[..2].to_owned()),
        by_month: group(&rows, |r| r.date[..7].to_owned()),
        by_exit: group(&rows, |r| r.exit_reason.to_owned()),
        first_half: stats(&first),
        second_half: stats(&second),
        config: config.clone(),
    };
    (report, rows)
}

fn csv(rows: &[TradeRow]) -> String {
    let mut out = String::from(
        "date,side,signal_time,entry_time,entry,stop,target,risk_points,exit_time,exit_reason,exit_price,bars_held,max_r,gross_points,net_points,net_rupees,volume_ratio,atr,reason\n",
    );
    for r in rows {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},\"{}\"",
            r.date, r.side, r.signal_time, r.entry_time, r.entry, r.stop, r.target, r.risk_points, r.exit_time,
            r.exit_reason, r.exit_price, r.bars_held, r.max_r, r.gross_points, r.net_points, r.net_rupees,
            r.volume_ratio, r.atr, r.reason.replace('"', "'")
        );
    }
    out
}

fn table(title: &str, groups: &BTreeMap<String, Stats>) -> String {
    let mut out = format!("\n{title}\n  {:<10} {:>6} {:>7} {:>9} {:>10} {:>6}\n", "", "trades", "win%", "net pts", "net ₹", "PF");
    for (k, s) in groups {
        let _ = writeln!(
            out,
            "  {:<10} {:>6} {:>7.1} {:>9.1} {:>10.0} {:>6.2}",
            k, s.trades, s.win_rate, s.net_points, s.net_rupees, s.profit_factor
        );
    }
    out
}

pub fn run(config_path: &str, from: &str, to: &str) -> Result<()> {
    let config = BurstConfig::load(config_path)?;
    let from: NaiveDate = from.parse()?;
    let to: NaiveDate = to.parse()?;
    ensure!(from <= to, "FROM must not be after TO");
    let candles = fetch(config.instrument_token, from - Duration::days(WARMUP_DAYS), to)?;
    let bars: Vec<Bar> = bars_from(&candles)?.into_iter().filter(|b| b.session_date() <= to).collect();
    ensure!(!bars.is_empty(), "No 1m history for {} in the range", config.symbol);
    let (report, rows) = report(&config, &bars, from, to);

    let dir = PathBuf::from("backtest_results")
        .join(STRATEGY)
        .join(&config.symbol)
        .join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
    std::fs::create_dir_all(&dir)?;
    backtest_report::json(&dir, "summary.json", &report)?;
    std::fs::write(dir.join("trades.csv"), csv(&rows))?;

    let o = &report.overall;
    let mut text = format!(
        "CRUDE-BURST {} {}..{}  ({} days, {} bars, {} signals)\n\
         trades {}  win {:.1}%  net {:.1} pts = ₹{:.0}  PF {:.2}  exp {:.2} pts/trade  avg win {:.1} / loss {:.1}  max DD ₹{:.0}\n\
         first half: {} trades ₹{:.0} PF {:.2}   second half: {} trades ₹{:.0} PF {:.2}",
        report.symbol, from, to, report.trading_days, report.bars, report.signals,
        o.trades, o.win_rate, o.net_points, o.net_rupees, o.profit_factor, o.expectancy_points,
        o.avg_win_points, o.avg_loss_points, o.max_drawdown_rupees,
        report.first_half.trades, report.first_half.net_rupees, report.first_half.profit_factor,
        report.second_half.trades, report.second_half.net_rupees, report.second_half.profit_factor,
    );
    text.push_str(&table("By side", &report.by_side));
    text.push_str(&table("By exit", &report.by_exit));
    text.push_str(&table("By entry hour (IST)", &report.by_hour_ist));
    text.push_str(&table("By month", &report.by_month));
    let _ = writeln!(text, "\nRejected bars (first failing rule): {:?}\nSkipped signals: {:?}\nOutput: {}", report.rejected_bars, report.skipped_signals, dir.display());
    std::fs::write(dir.join("summary.txt"), &text)?;
    println!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    /// Shipped costs and session, library-default rules (independent of tuning).
    fn config() -> BurstConfig {
        let mut c: BurstConfig = serde_json::from_str(include_str!("../../../../config/burst-crudeoilm.json")).unwrap();
        c.params = burst::Params::default();
        c
    }
    fn b(start: i64, o: f64, h: f64, l: f64, c: f64, v: f64) -> Bar {
        Bar { start, seconds: 60, open: o, high: h, low: l, close: c, volume: v }
    }
    #[test]
    fn replay_enters_next_open_and_charges_costs_and_slippage() {
        let t0 = Utc.with_ymd_and_hms(2026, 10, 9, 3, 30, 0).unwrap().timestamp();
        let mut bars: Vec<Bar> = (0..30).map(|i| b(t0 + i * 60, 101.0, 102.0, 100.0, 101.0, 10.0)).collect();
        bars.push(b(t0 + 30 * 60, 102.0, 108.0, 101.5, 107.5, 50.0)); // signal, stop 99
        bars.push(b(t0 + 31 * 60, 107.0, 108.0, 106.0, 107.5, 20.0)); // entry 107 + 1 slip = 108, risk 9
        bars.push(b(t0 + 32 * 60, 108.0, 122.0, 107.5, 121.0, 30.0)); // target 108 + 13.5 = 121.5 hit
        let c = config();
        let (rows, signals, _, _) = replay(&c, &bars, NaiveDate::from_ymd_opt(2026, 10, 9).unwrap());
        assert_eq!(signals, 1);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!((r.side, r.entry, r.risk_points, r.exit_reason), ("LONG", 108.0, 9.0, "TARGET"));
        assert_eq!(r.exit_price, 120.5, "target 121.5 minus 1 slippage");
        assert_eq!(r.net_points, 12.5 - 6.0);
        assert_eq!(r.net_rupees, 65.0);
        assert_eq!((r.signal_time.as_str(), r.entry_time.as_str()), ("09:30", "09:31"));
    }
    #[test]
    fn open_trade_is_squared_off_at_the_last_bar_of_the_day() {
        let t0 = Utc.with_ymd_and_hms(2026, 10, 9, 3, 30, 0).unwrap().timestamp();
        let mut bars: Vec<Bar> = (0..30).map(|i| b(t0 + i * 60, 101.0, 102.0, 100.0, 101.0, 10.0)).collect();
        bars.push(b(t0 + 30 * 60, 102.0, 108.0, 101.5, 107.5, 50.0));
        bars.push(b(t0 + 31 * 60, 107.0, 112.0, 106.0, 111.0, 20.0)); // last bar of the data/day
        let (rows, _, _, _) = replay(&config(), &bars, NaiveDate::from_ymd_opt(2026, 10, 9).unwrap());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].exit_reason, "SQUARE_OFF");
    }
}

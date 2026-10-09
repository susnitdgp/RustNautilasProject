//! Read-only Precision Sniper replay on Kite history with MIS execution. Never places orders.
//!
//! The model (crate `sniper`) runs bar for bar exactly like the Pine script: entry at the
//! signal bar close, exits stop-first on bar OHLC, reversal on an accepted opposite signal.
//! The execution layer adds what a Zerodha MIS account needs and the script does not
//! model: no new entries from `entries_until`, square-off at the first bar close at/after
//! `square_off` (or the day's last bar), adverse slippage on every fill and an all-in
//! round-trip cost per lot.
use super::backtest_report;
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveTime};
use kite_adapter::http::historical::{Candle, Interval, Reader};
use serde::{Deserialize, Serialize};
use sniper::{Bar, Engine, Event};
use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf};

pub const STRATEGY: &str = "sniper";
/// Calendar days fetched before FROM for warm-up (EMA trend × 3, ATR mean, MACD).
const WARMUP_DAYS: i64 = 4;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SniperConfig {
    pub strategy: String,
    pub symbol: String,
    pub instrument_token: u32,
    /// Candle minutes: 3, 5, 10, 15 or 30 (fetched natively from Kite).
    pub bar_minutes: u32,
    pub lots: u32,
    pub point_value: f64,
    pub round_trip_cost_points: f64,
    pub slippage_points_per_side: f64,
    pub entries_until: NaiveTime,
    pub square_off: NaiveTime,
    #[serde(default)]
    pub params: sniper::Params,
}

impl SniperConfig {
    pub fn load(path: &str) -> Result<Self> {
        let raw = std::fs::read(path).with_context(|| format!("Cannot read {path}"))?;
        let c: Self = serde_json::from_slice(&raw).with_context(|| format!("Invalid sniper config {path}"))?;
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.strategy == STRATEGY, "strategy must be \"{STRATEGY}\"");
        ensure!(self.instrument_token > 0, "instrument_token required");
        ensure!([3, 5, 10, 15, 30].contains(&self.bar_minutes), "bar_minutes must be 3, 5, 10, 15 or 30");
        ensure!(self.params.timeframe_minutes == self.bar_minutes, "params.timeframe_minutes must equal bar_minutes");
        ensure!((1..=100).contains(&self.lots), "lots must be 1..100");
        ensure!(self.point_value > 0.0, "point_value must be positive");
        ensure!(self.round_trip_cost_points >= 0.0 && self.slippage_points_per_side >= 0.0, "costs must be >= 0");
        ensure!(self.entries_until <= self.square_off, "entries_until must not be after square_off");
        self.params.validate().map_err(anyhow::Error::msg)
    }
    fn interval(&self) -> Interval {
        use kite_adapter::http::historical::KiteInterval as K;
        match self.bar_minutes {
            3 => K::ThreeMinute.native(),
            10 => K::TenMinute.native(),
            15 => K::FifteenMinute.native(),
            30 => K::ThirtyMinute.native(),
            _ => K::FiveMinute.native(),
        }
    }
}

fn ist(ts: i64) -> DateTime<FixedOffset> {
    DateTime::from_timestamp(ts, 0).expect("timestamp").with_timezone(&FixedOffset::east_opt(19_800).expect("IST"))
}
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub date: String,
    pub side: &'static str,
    pub entry_time: String,
    pub exit_time: String,
    pub grade: &'static str,
    pub score: f64,
    pub model_entry: f64,
    pub stop: f64,
    pub tp1: f64,
    pub tp3: f64,
    pub exit_reason: &'static str,
    pub model_exit: f64,
    pub model_r: f64,
    pub exec_entry: f64,
    pub exec_exit: f64,
    pub net_points: f64,
    pub net_rupees: f64,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
pub struct Stats {
    pub trades: usize,
    pub wins: usize,
    pub win_rate: f64,
    pub net_rupees: f64,
    pub net_points: f64,
    pub profit_factor: f64,
    pub avg_win_rupees: f64,
    pub avg_loss_rupees: f64,
    pub model_r: f64,
    pub max_drawdown_rupees: f64,
}

pub fn stats(rows: &[&Row]) -> Stats {
    if rows.is_empty() {
        return Stats::default();
    }
    let wins: Vec<f64> = rows.iter().map(|r| r.net_rupees).filter(|v| *v > 0.0).collect();
    let losses: Vec<f64> = rows.iter().map(|r| r.net_rupees).filter(|v| *v <= 0.0).collect();
    let gw = wins.iter().fold(0.0, |a, b| a + b);
    let gl = losses.iter().fold(0.0, |a, b| a - b);
    let (mut eq, mut peak, mut dd) = (0.0_f64, 0.0_f64, 0.0_f64);
    for r in rows {
        eq += r.net_rupees;
        peak = peak.max(eq);
        dd = dd.max(peak - eq);
    }
    Stats {
        trades: rows.len(),
        wins: wins.len(),
        win_rate: round2(wins.len() as f64 * 100.0 / rows.len() as f64),
        net_rupees: round2(rows.iter().fold(0.0, |a, r| a + r.net_rupees)),
        net_points: round2(rows.iter().fold(0.0, |a, r| a + r.net_points)),
        profit_factor: round2(if gl > 0.0 { gw / gl } else { 999.0 }),
        avg_win_rupees: round2(if wins.is_empty() { 0.0 } else { gw / wins.len() as f64 }),
        avg_loss_rupees: round2(if losses.is_empty() { 0.0 } else { -gl / losses.len() as f64 }),
        model_r: round2(rows.iter().fold(0.0, |a, r| a + r.model_r)),
        max_drawdown_rupees: round2(dd),
    }
}

struct Open {
    dir: i32,
    entry_ts: i64,
    entry: f64,
    stop: f64,
    tp1: f64,
    tp3: f64,
    grade: &'static str,
    score: f64,
}

/// Replays bars (oldest first, `seconds` long). Bars before `from` only warm the model.
pub fn replay(c: &SniperConfig, bars: &[Bar], from: NaiveDate) -> (Vec<Row>, Engine) {
    let mut engine = Engine::new(c.params.clone());
    let step = c.bar_minutes as i64 * 60;
    let slip = c.slippage_points_per_side;
    let mut open: Option<Open> = None;
    let mut rows = Vec::new();
    let close_row = |o: Open, ts: i64, px: f64, reason: &'static str, model_r: f64, rows: &mut Vec<Row>| {
        let d = o.dir as f64;
        let exec_entry = o.entry + d * slip;
        let exec_exit = px - d * slip;
        let net = d * (exec_exit - exec_entry) - c.round_trip_cost_points;
        rows.push(Row {
            date: ist(o.entry_ts).date_naive().to_string(),
            side: if o.dir == 1 { "LONG" } else { "SHORT" },
            entry_time: ist(o.entry_ts).format("%H:%M").to_string(),
            exit_time: ist(ts).format("%H:%M").to_string(),
            grade: o.grade,
            score: o.score,
            model_entry: o.entry,
            stop: o.stop,
            tp1: o.tp1,
            tp3: o.tp3,
            exit_reason: reason,
            model_exit: px,
            model_r: round2(model_r),
            exec_entry,
            exec_exit,
            net_points: round2(net),
            net_rupees: round2(net * c.point_value * c.lots as f64),
        });
    };
    for (i, b) in bars.iter().enumerate() {
        let close_ts = b.start + step;
        let close_t = ist(close_ts).time();
        let trading = ist(b.start).date_naive() >= from;
        let last_of_day = bars.get(i + 1).is_none_or(|n| ist(n.start).date_naive() != ist(b.start).date_naive());
        let allowed = trading && close_t < c.entries_until && close_t >= NaiveTime::from_hms_opt(9, 0, 0).expect("t") && !last_of_day;
        for ev in engine.on_bar(*b, allowed) {
            match ev {
                Event::Exit { price, reason, gross_r, .. } => {
                    if let Some(o) = open.take() {
                        close_row(o, close_ts, price, reason, gross_r, &mut rows);
                    }
                }
                Event::Entry { dir, price, stop, tp1, tp3, grade, score, .. } => {
                    open = Some(Open { dir, entry_ts: close_ts, entry: price, stop, tp1, tp3, grade, score });
                }
            }
        }
        if (close_t >= c.square_off || last_of_day)
            && let Some(Event::Exit { price, reason, gross_r, .. }) = engine.force_close(b.close, "Square-off")
            && let Some(o) = open.take()
        {
            close_row(o, close_ts, price, reason, gross_r, &mut rows);
        }
    }
    (rows, engine)
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub strategy: &'static str,
    pub symbol: String,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub trading_days: usize,
    pub preset: String,
    pub overall: Stats,
    pub daily: BTreeMap<String, Stats>,
    pub by_side: BTreeMap<String, Stats>,
    pub by_exit: BTreeMap<String, Stats>,
    pub by_grade: BTreeMap<String, Stats>,
    pub by_hour_ist: BTreeMap<String, Stats>,
    pub candidates: u32,
    pub rejected: BTreeMap<String, u32>,
    pub config: SniperConfig,
}

fn group(rows: &[Row], key: impl Fn(&Row) -> String) -> BTreeMap<String, Stats> {
    let mut m: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
    for r in rows {
        m.entry(key(r)).or_default().push(r);
    }
    m.into_iter().map(|(k, v)| (k, stats(&v))).collect()
}

pub fn report(c: &SniperConfig, bars: &[Bar], from: NaiveDate, to: NaiveDate) -> (Report, Vec<Row>) {
    let (rows, engine) = replay(c, bars, from);
    let mut days: Vec<NaiveDate> = bars.iter().map(|b| ist(b.start).date_naive()).filter(|d| *d >= from && *d <= to).collect();
    days.dedup();
    let all: Vec<&Row> = rows.iter().collect();
    let rejected = engine
        .rejects
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(code, n)| (sniper::engine::reject_reason(code as u8).to_owned(), *n))
        .collect();
    let report = Report {
        strategy: STRATEGY,
        symbol: c.symbol.clone(),
        from,
        to,
        trading_days: days.len(),
        preset: format!("{:?}", engine.resolved()),
        overall: stats(&all),
        daily: group(&rows, |r| r.date.clone()),
        by_side: group(&rows, |r| r.side.to_owned()),
        by_exit: group(&rows, |r| r.exit_reason.to_owned()),
        by_grade: group(&rows, |r| r.grade.to_owned()),
        by_hour_ist: group(&rows, |r| r.entry_time[..2].to_owned()),
        candidates: engine.candidates,
        rejected,
        config: c.clone(),
    };
    (report, rows)
}

pub fn bars_from(candles: &[Candle]) -> Result<Vec<Bar>> {
    candles
        .iter()
        .map(|k| {
            Ok(Bar { start: k.time()?.timestamp(), open: k.open, high: k.high, low: k.low, close: k.close, volume: k.volume as f64 })
        })
        .collect()
}

fn fetch(token: u32, first: NaiveDate, last: NaiveDate, interval: Interval) -> Result<Vec<Candle>> {
    let rt = tokio::runtime::Runtime::new()?;
    let mut candles = rt.block_on(async {
        let mut reader = Reader::default();
        let mut all = Vec::new();
        let mut start = first;
        while start <= last {
            let end = (start + Duration::days(25)).min(last);
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

fn table(title: &str, g: &BTreeMap<String, Stats>) -> String {
    let mut out = format!("\n{title}\n  {:<12} {:>6} {:>6} {:>9} {:>8} {:>6} {:>7}\n", "", "trades", "win%", "net ₹", "net pts", "PF", "model R");
    for (k, s) in g {
        let _ = writeln!(
            out,
            "  {:<12} {:>6} {:>6.1} {:>9.0} {:>8.1} {:>6.2} {:>7.2}",
            k, s.trades, s.win_rate, s.net_rupees, s.net_points, s.profit_factor, s.model_r
        );
    }
    out
}

pub fn run(config_path: &str, from: &str, to: &str) -> Result<()> {
    let c = SniperConfig::load(config_path)?;
    let from: NaiveDate = from.parse()?;
    let to: NaiveDate = to.parse()?;
    ensure!(from <= to, "FROM must not be after TO");
    let candles = fetch(c.instrument_token, from - Duration::days(WARMUP_DAYS), to, c.interval())?;
    let bars: Vec<Bar> = bars_from(&candles)?.into_iter().filter(|b| ist(b.start).date_naive() <= to).collect();
    ensure!(!bars.is_empty(), "No history for {} in the range", c.symbol);
    let (report, rows) = report(&c, &bars, from, to);

    let dir = PathBuf::from("backtest_results")
        .join(STRATEGY)
        .join(&c.symbol)
        .join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
    std::fs::create_dir_all(&dir)?;
    backtest_report::json(&dir, "summary.json", &report)?;
    let mut csv = String::from("date,side,entry_time,exit_time,grade,score,model_entry,stop,tp1,tp3,exit_reason,model_exit,model_r,exec_entry,exec_exit,net_points,net_rupees\n");
    for r in &rows {
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            r.date, r.side, r.entry_time, r.exit_time, r.grade, r.score, r.model_entry, r.stop, r.tp1, r.tp3, r.exit_reason,
            r.model_exit, r.model_r, r.exec_entry, r.exec_exit, r.net_points, r.net_rupees
        );
    }
    std::fs::write(dir.join("trades.csv"), csv)?;
    let o = &report.overall;
    let mut text = format!(
        "PRECISION SNIPER {} {}..{} ({} days)  {}\n\
         trades {}  win {:.1}%  net ₹{:.0} ({:.1} pts)  PF {:.2}  avg win ₹{:.0} / loss ₹{:.0}  max DD ₹{:.0}  model {:.2} R (no costs)\n\
         costs: {} pts round trip + {} pt slippage per fill, {} lot(s) × {} ₹/pt; entries until {}, square-off {}",
        report.symbol, from, to, report.trading_days, report.preset,
        o.trades, o.win_rate, o.net_rupees, o.net_points, o.profit_factor, o.avg_win_rupees, o.avg_loss_rupees, o.max_drawdown_rupees, o.model_r,
        c.round_trip_cost_points, c.slippage_points_per_side, c.lots, c.point_value, c.entries_until, c.square_off,
    );
    text.push_str(&table("Daily", &report.daily));
    text.push_str(&table("By side", &report.by_side));
    text.push_str(&table("By exit", &report.by_exit));
    text.push_str(&table("By grade", &report.by_grade));
    text.push_str(&table("By entry hour (IST)", &report.by_hour_ist));
    let _ = writeln!(text, "\nCrossover candidates {}; rejected: {:?}\nOutput: {}", report.candidates, report.rejected, dir.display());
    std::fs::write(dir.join("summary.txt"), &text)?;
    println!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> SniperConfig {
        serde_json::from_str(include_str!("../../../../config/sniper-crudeoilm.json")).unwrap()
    }
    #[test]
    fn shipped_config_is_valid() {
        let c = config();
        c.validate().unwrap();
        assert_eq!(c.params.resolve().preset, sniper::params::Preset::Scalping);
        let mut bad = c.clone();
        bad.params.timeframe_minutes = 15;
        assert!(bad.validate().is_err(), "chart timeframe must match bar_minutes");
    }
    /// Synthetic 5m days 09:00-23:30 IST: every trade pays slippage + costs, no position
    /// survives the square-off and no entry is taken at/after `entries_until`.
    #[test]
    fn replay_charges_costs_and_squares_off_daily() {
        let c = config();
        let day0 = chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        let mut bars = Vec::new();
        let mut px = 8000.0_f64;
        let mut n = 0_i64;
        for day in 0..12 {
            let open_ts = (day0 + Duration::days(day)).and_hms_opt(3, 30, 0).unwrap().and_utc().timestamp();
            for k in 0..174_i64 {
                let drift = (n as f64 / 23.0).sin() * 7.0 + (n as f64 / 7.0).cos() * 3.0;
                let o = px;
                let cl = (px + drift).round();
                bars.push(Bar { start: open_ts + k * 300, open: o, high: o.max(cl) + 3.0, low: o.min(cl) - 3.0, close: cl, volume: 100.0 + ((n * 31) % 80) as f64 });
                px = cl;
                n += 1;
            }
        }
        let (rows, _) = replay(&c, &bars, day0);
        assert!(!rows.is_empty(), "synthetic market should trade");
        for r in &rows {
            let d = if r.side == "LONG" { 1.0 } else { -1.0 };
            assert_eq!(r.exec_entry, r.model_entry + d * c.slippage_points_per_side);
            assert_eq!(r.exec_exit, r.model_exit - d * c.slippage_points_per_side);
            assert!((r.net_points - (d * (r.exec_exit - r.exec_entry) - c.round_trip_cost_points)).abs() < 0.011);
            assert!(r.entry_time.as_str() < "23:00", "entry at {}", r.entry_time);
            assert!(r.exit_time.as_str() <= "23:30" && r.exit_time.as_str() >= r.entry_time.as_str(), "overnight trade {:?}", r);
        }
    }
}

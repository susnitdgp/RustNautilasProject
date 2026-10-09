//! Read-only CRUDE-BURST parameter sweep: one Kite fetch, a fixed grid of the key
//! rules, each combination replayed exactly like `native-burst-backtest`.
//!
//! Over-fitting guard: a combination is only "robust" when BOTH halves of the
//! period are profitable after costs; results are sorted robust-first, then by
//! the weaker half's profit (not the best total).
use super::{
    burst_backtest::{self, Stats, TradeRow},
    burst_config::{BurstConfig, STRATEGY},
};
use anyhow::{Result, ensure};
use burst::Bar;
use chrono::{Duration, NaiveDate};
use serde::Serialize;
use std::{fmt::Write as _, path::PathBuf};

#[derive(Debug, Serialize)]
pub struct Row {
    pub base_max_atr: f64,
    pub breakout_min_atr: f64,
    pub volume_mult: f64,
    pub room_mult: f64,
    pub target_r: f64,
    pub trades: usize,
    pub win_rate: f64,
    pub net_rupees: f64,
    pub profit_factor: f64,
    pub expectancy_points: f64,
    pub max_drawdown_rupees: f64,
    pub first_half_rupees: f64,
    pub second_half_rupees: f64,
    pub robust: bool,
}

/// The grid: 4 × 2 × 3 × 2 × 2 = 96 combinations.
pub fn grid(base: &BurstConfig) -> Vec<BurstConfig> {
    let mut out = Vec::new();
    for base_max_atr in [2.0, 2.5, 3.0, 3.5] {
        for breakout_min_atr in [1.0, 1.5] {
            for volume_mult in [1.5, 2.0, 2.5] {
                for room_mult in [0.0, 1.0] {
                    for target_r in [1.5, 2.0] {
                        let mut c = base.clone();
                        c.params.base_max_atr = base_max_atr;
                        c.params.breakout_min_atr = breakout_min_atr;
                        c.params.volume_mult = volume_mult;
                        c.params.room_mult = room_mult;
                        c.params.target_r = target_r;
                        out.push(c);
                    }
                }
            }
        }
    }
    out
}

pub fn evaluate(c: &BurstConfig, bars: &[Bar], from: NaiveDate, to: NaiveDate) -> Row {
    let (report, _rows): (burst_backtest::Report, Vec<TradeRow>) = burst_backtest::report(c, bars, from, to);
    let o: &Stats = &report.overall;
    let (h1, h2) = (report.first_half.net_rupees, report.second_half.net_rupees);
    Row {
        base_max_atr: c.params.base_max_atr,
        breakout_min_atr: c.params.breakout_min_atr,
        volume_mult: c.params.volume_mult,
        room_mult: c.params.room_mult,
        target_r: c.params.target_r,
        trades: o.trades,
        win_rate: o.win_rate,
        net_rupees: o.net_rupees,
        profit_factor: o.profit_factor,
        expectancy_points: o.expectancy_points,
        max_drawdown_rupees: o.max_drawdown_rupees,
        first_half_rupees: h1,
        second_half_rupees: h2,
        robust: h1 > 0.0 && h2 > 0.0 && o.trades >= 20,
    }
}

pub fn sort(rows: &mut [Row]) {
    rows.sort_by(|a, b| {
        b.robust
            .cmp(&a.robust)
            .then(b.first_half_rupees.min(b.second_half_rupees).total_cmp(&a.first_half_rupees.min(a.second_half_rupees)))
    });
}

pub fn run(config_path: &str, from: &str, to: &str) -> Result<()> {
    let base = BurstConfig::load(config_path)?;
    let from: NaiveDate = from.parse()?;
    let to: NaiveDate = to.parse()?;
    ensure!(from <= to, "FROM must not be after TO");
    let candles = burst_backtest::fetch(base.instrument_token, from - Duration::days(burst_backtest::WARMUP_DAYS), to)?;
    let bars: Vec<Bar> = burst_backtest::bars_from(&candles)?.into_iter().filter(|b| b.session_date() <= to).collect();
    ensure!(!bars.is_empty(), "No 1m history for {} in the range", base.symbol);
    let mut rows: Vec<Row> = grid(&base).iter().map(|c| evaluate(c, &bars, from, to)).collect();
    sort(&mut rows);

    let dir = PathBuf::from("backtest_results")
        .join(STRATEGY)
        .join(&base.symbol)
        .join(format!("sweep-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    std::fs::create_dir_all(&dir)?;
    let mut csv = String::from(
        "base_max_atr,breakout_min_atr,volume_mult,room_mult,target_r,trades,win_rate,net_rupees,profit_factor,expectancy_points,max_drawdown_rupees,first_half_rupees,second_half_rupees,robust\n",
    );
    for r in &rows {
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            r.base_max_atr, r.breakout_min_atr, r.volume_mult, r.room_mult, r.target_r, r.trades, r.win_rate,
            r.net_rupees, r.profit_factor, r.expectancy_points, r.max_drawdown_rupees, r.first_half_rupees,
            r.second_half_rupees, r.robust
        );
    }
    std::fs::write(dir.join("sweep.csv"), &csv)?;
    let robust = rows.iter().filter(|r| r.robust).count();
    let profitable = rows.iter().filter(|r| r.net_rupees > 0.0).count();
    let mut text = format!(
        "CRUDE-BURST sweep {} {}..{}: {} combinations, {} profitable overall, {} robust (both halves > 0, >= 20 trades)\n\
         {:>5} {:>5} {:>4} {:>4} {:>4} | {:>6} {:>6} {:>9} {:>5} {:>6} {:>8} | {:>8} {:>8}\n",
        base.symbol, from, to, rows.len(), profitable, robust,
        "base", "bar", "vol", "room", "tgt", "trades", "win%", "net ₹", "PF", "exp", "maxDD", "half1 ₹", "half2 ₹",
    );
    for r in rows.iter().take(25) {
        let _ = writeln!(
            text,
            "{:>5} {:>5} {:>4} {:>4} {:>4} | {:>6} {:>6.1} {:>9.0} {:>5.2} {:>6.2} {:>8.0} | {:>8.0} {:>8.0}{}",
            r.base_max_atr, r.breakout_min_atr, r.volume_mult, r.room_mult, r.target_r, r.trades, r.win_rate,
            r.net_rupees, r.profit_factor, r.expectancy_points, r.max_drawdown_rupees, r.first_half_rupees,
            r.second_half_rupees, if r.robust { "  *" } else { "" }
        );
    }
    let _ = writeln!(text, "Output: {}", dir.display());
    std::fs::write(dir.join("sweep.txt"), &text)?;
    println!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_has_96_distinct_valid_configs_and_sort_prefers_robust() {
        let base: BurstConfig = serde_json::from_str(include_str!("../../../../config/burst-crudeoilm.json")).unwrap();
        let g = grid(&base);
        assert_eq!(g.len(), 96);
        for c in &g {
            c.validate().unwrap();
        }
        let mk = |h1: f64, h2: f64, trades: usize| Row {
            base_max_atr: 2.0, breakout_min_atr: 1.0, volume_mult: 2.0, room_mult: 0.0, target_r: 1.5,
            trades, win_rate: 50.0, net_rupees: h1 + h2, profit_factor: 1.0, expectancy_points: 0.0,
            max_drawdown_rupees: 0.0, first_half_rupees: h1, second_half_rupees: h2,
            robust: h1 > 0.0 && h2 > 0.0 && trades >= 20,
        };
        let mut rows = vec![mk(5000.0, -100.0, 40), mk(300.0, 200.0, 30), mk(900.0, 800.0, 30)];
        sort(&mut rows);
        assert_eq!((rows[0].first_half_rupees, rows[1].first_half_rupees), (900.0, 300.0));
        assert!(!rows[2].robust, "big total with a losing half ranks last");
    }
}

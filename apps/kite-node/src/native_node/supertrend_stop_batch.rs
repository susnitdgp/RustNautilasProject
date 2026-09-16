//! Offline confirmed Supertrend comparison with and without a fixed ATR stop.
use super::{backtest_report as report, supertrend_input, vwap_input, vwap_report};
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use nautilus_core::UUID4;
use serde_json::{Value, json};
use std::path::Path;
pub fn run(start: &str, end: &str, path: &str) -> Result<()> {
    let start = NaiveDate::parse_from_str(start, "%Y-%m-%d")?;
    let end = NaiveDate::parse_from_str(end, "%Y-%m-%d")?;
    let days = vwap_input::range(start, end)?;
    let folder = report::directory(
        &format!("supertrend_stop_comparison_{start}_to_{end}"),
        &UUID4::new().to_string(),
    )?;
    println!("Backtest results: {}", folder.display());
    let result = execute(start, end, path, &days, &folder);
    if let Err(error) = &result {
        report::json(
            &folder,
            "failure.json",
            &json!({"status":"failed","error":error.to_string(),"live_orders_enabled":false}),
        )?;
    }
    result.map(|v| println!("{v}"))
}
fn execute(
    start: NaiveDate,
    end: NaiveDate,
    path: &str,
    days: &[NaiveDate],
    folder: &Path,
) -> Result<Value> {
    let data = supertrend_input::load(end, Some(path))?;
    // Validate every requested session before starting any engine.
    for date in days {
        let session = supertrend_input::Input {
            instrument_id: data.instrument_id.clone(),
            instrument_token: data.instrument_token,
            interval: data.interval.clone(),
            source: data.source.clone(),
            candles: data
                .candles
                .iter()
                .filter(|c| c.time().is_ok_and(|t| t.date_naive() <= *date))
                .cloned()
                .collect(),
        };
        vwap_input::validate(&session, *date)?;
        report::json(folder, &format!("{date}_input.json"), &session)?;
    }
    std::fs::copy(path, folder.join("historical_input.json"))?;
    let key = format!("kite:backtest:supertrend-stop:{}:cooldown", UUID4::new());
    let mut con = redis::Client::open(kite_journal::connection::url_from_env()?.as_str())?
        .get_connection()?;
    let created: Option<String> = redis::cmd("SET")
        .arg(&key)
        .arg(0)
        .arg("NX")
        .query(&mut con)?;
    ensure!(created.is_some(), "Cooldown namespace collision");
    let mut rows = Vec::new();
    for variant in ["confirmed", "atr_stop"] {
        let dir = folder.join(variant);
        std::fs::create_dir(&dir)?;
        let mut daily = Vec::new();
        let mut trades = Vec::new();
        for date in days {
            let child = dir.join(date.to_string());
            let output = std::process::Command::new(std::env::current_exe()?)
                .arg(if variant == "atr_stop" {
                    "native-supertrend-stop-session"
                } else {
                    "native-supertrend-session"
                })
                .arg(date.to_string())
                .arg(folder.join(format!("{date}_input.json")))
                .arg(&child)
                .arg(if variant == "atr_stop" {
                    key.as_str()
                } else {
                    "confirmed"
                })
                .output()?;
            let logs = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            std::fs::write(dir.join(format!("{date}.log")), &logs)?;
            ensure!(
                output.status.success() && !logs.contains("[ERROR]"),
                "Failed {variant} {date}; inspect saved log"
            );
            let summary: Value =
                serde_json::from_slice(&std::fs::read(child.join("summary.json"))?)?;
            ensure!(
                summary["status"] == "completed" && summary["open_contracts"].as_f64() == Some(0.),
                "Non-flat/incomplete session"
            );
            let mut day_trades: Vec<Value> =
                serde_json::from_slice(&std::fs::read(child.join("trades.json"))?)?;
            for trade in &mut day_trades {
                trade["date_ist"] = json!(date.to_string());
            }
            trades.extend(day_trades);
            daily.push(json!({"date_ist":date.to_string(),"bars":summary["session_bars"],"fills":summary["fills"],"statistics":summary["statistics"]}));
        }
        let stats = vwap_report::statistics(&trades);
        report::json(&dir, "trades.json", &trades)?;
        let row = json!({"variant":variant,"statistics":stats,"sessions":daily});
        report::json(&dir, "summary.json", &row)?;
        rows.push(row);
    }
    let output = json!({"status":"completed","start_date_ist":start.to_string(),"end_date_ist":end.to_string(),"sessions":days.len(),
        "instrument":data.instrument_id,"interval":"5minute","contracts":1,"contract_multiplier":100,
        "supertrend":"ATR(7) Nautilus Wilder, multiplier 2; custom bands",
        "confirmation":"MACD(12,26) vs EMA(9) signal plus close vs session HLC3 VWAP; strict direction; no zero-line or EMA crossover filter",
        "entry":"When flat, current Supertrend direction and confirmations agree on completed bar; next-open fill; delayed confirmation allowed; no prior-day VWAP at new-session open",
        "exit":"Both: Supertrend reversal or session close. atr_stop also exits on fixed ATR stop; confirmation loss alone does not exit",
        "stop_loss_target":"atr_stop: fixed 1.5 ATR(14) Wilder from actual fill rounded outward; native stop-market. confirmed: unchanged no fixed stop",
        "cooldown_redis_key":key,"reentry":"atr_stop waits for a fresh Supertrend direction change after stop; persisted across sessions in Redis",
        "costs":"fees spread slippage excluded","drawdown":"closed-trade equity only","variants":rows,
        "default_strategy":"unchanged","live_orders_enabled":false,"broker_orders_sent":false});
    report::json(folder, "comparison.json", &output)?;
    let mut md = format!(
        "# Supertrend + MACD + VWAP: fixed-stop comparison\n\n{start} through {end}; {} sessions; 5-minute September CRUDEOIL; one lot.\n\n| Variant | Trades | Wins | Gross INR | Closed-trade drawdown INR |\n|---|---:|---:|---:|---:|\n",
        days.len()
    );
    for r in &rows {
        let s = &r["statistics"];
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            r["variant"].as_str().unwrap(),
            s["trades"],
            s["wins"],
            s["gross_pnl_inr"],
            s["max_closed_trade_drawdown_inr"]
        ));
    }
    md.push_str("\nSupertrend (7,2) uses native ATR Wilder and custom bands. Confirmed long: bullish Supertrend, close above session VWAP, MACD above signal. Short reverses all three. All inequalities strict. MACD is 12/26 with EMA9 signal. No MACD zero-line test or EMA crossover. Wait for confirmation if initially absent. No previous-day VWAP entry at session open.\n\nHold until a completed-bar Supertrend reversal or session close; loss of confirmation alone does not exit. Stop variant adds a fixed 1.5 ATR(14) stop from actual entry and blocks entries until a fresh Supertrend direction change after a stop. Cooldown persists across sessions in Redis. No profit target. Entries/exits use next-open simulated market fills, except session-close liquidation. Costs excluded. Original VWAP strategy used different exits, so this is not an isolated entry-filter comparison against that strategy. No parameters optimized and no default changed.\n\nExact input, per-session candles, indicators, signals, fills, trades and native results are saved. Read-only offline replay; real orders disabled. This is retrospective testing, not forward validation.\n");
    std::fs::write(folder.join("README.md"), md)?;
    Ok(output)
}

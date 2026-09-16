//! Seven trading sessions with isolated native engines and a combined report.
use super::{
    backtest_report as report,
    supertrend_input::{self, Input},
    vwap_input, vwap_report,
};
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use nautilus_core::UUID4;
use serde_json::{Value, json};
use std::path::Path;
pub fn run(end: &str, path: Option<&str>) -> Result<()> {
    let end = NaiveDate::parse_from_str(end, "%Y-%m-%d")?;
    let days = vwap_input::dates(end)?;
    let folder = report::directory(
        &format!("vwap_ema_macd_5minute_7sessions_{end}"),
        &UUID4::new().to_string(),
    )?;
    println!("Backtest results: {}", folder.display());
    let result = execute(end, path, &days, &folder);
    if let Err(error) = &result {
        report::json(
            &folder,
            "failure.json",
            &json!({"status":"failed","error":error.to_string(),"live_orders_enabled":false}),
        )?;
    }
    result.map(|value| println!("{value}"))
}
fn execute(end: NaiveDate, path: Option<&str>, days: &[NaiveDate], folder: &Path) -> Result<Value> {
    let data = supertrend_input::load_window(end, path, 30)?;
    kite_adapter::http::historical::validate(&data.candles)?;
    report::json(folder, "historical_input.json", &data)?;
    // Validate every requested day before starting any engine.
    let mut sessions = Vec::new();
    for date in days {
        let candles = data
            .candles
            .iter()
            .filter(|c| c.time().is_ok_and(|t| t.date_naive() <= *date))
            .cloned()
            .collect();
        let session = Input {
            instrument_id: data.instrument_id.clone(),
            instrument_token: data.instrument_token,
            interval: data.interval.clone(),
            source: data.source.clone(),
            candles,
        };
        vwap_input::validate(&session, *date)?;
        sessions.push((*date, session));
    }
    let mut daily = Vec::new();
    let mut trades = Vec::new();
    for (date, session) in sessions {
        let input_name = format!("{date}_input.json");
        report::json(folder, &input_name, &session)?;
        let child_folder = folder.join(date.to_string());
        let output = std::process::Command::new(std::env::current_exe()?)
            .arg("native-vwap-session")
            .arg(date.to_string())
            .arg(folder.join(&input_name))
            .arg(&child_folder)
            .output()?;
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::write(folder.join(format!("{date}.log")), &logs)?;
        ensure!(
            output.status.success() && !logs.contains("[ERROR]"),
            "Native session {date} failed; inspect its log"
        );
        let summary: Value =
            serde_json::from_slice(&std::fs::read(child_folder.join("summary.json"))?)?;
        ensure!(
            summary["status"] == "completed" && summary["open_contracts"].as_f64() == Some(0.),
            "Incomplete or non-flat session"
        );
        let mut day_trades: Vec<Value> =
            serde_json::from_slice(&std::fs::read(child_folder.join("trades.json"))?)?;
        for trade in &mut day_trades {
            trade["date_ist"] = json!(date.to_string());
        }
        daily.push(json!({"date_ist":date.to_string(),"bars":summary["session_bars"],"fills":summary["fills"],"statistics":summary["statistics"]}));
        trades.extend(day_trades);
    }
    let stats = vwap_report::statistics(&trades);
    report::json(folder, "trades.json", &trades)?;
    let summary = json!({"status":"completed","strategy":"VWAP + EMA(9/21) crossover + MACD(12/26/9), ATR(14) stop x1.5",
        "instrument":data.instrument_id,"interval":"5minute","data_source":data.source,
        "sessions":daily,"statistics":stats,"position_size_contracts":1,"contract_multiplier":100,
        "starting_equity_inr":1_000_000,"open_contracts":0,"fees_spread_slippage":"excluded",
        "capital_model":"fixed one-contract sizing; sum daily P&L; no compounding",
        "entry":"fresh crossover, close above/below session HLC3 VWAP and MACD above/below signal on same closed bar; next-open fill",
        "exit":"fixed ATR stop, fully confirmed opposite entry setup, or session close",
        "stop_fill_model":"native OHLC matching; intrabar timestamps are synthetic",
        "holiday_calendar":"2026-09-14 evening only, 17:00–23:30 IST",
        "native_indicators":true,"native_backtest_node":true,"native_redis_cache":true,
        "live_orders_enabled":false,"broker_orders_sent":false});
    report::json(folder, "summary.json", &summary)?;
    let mut md = format!(
        "# Seven-session VWAP / EMA / MACD backtest\n\nCRUDEOIL26SEPFUT, five-minute bars. Native EMA 9/21, MACD 12/26/9, ATR 14 Wilder, fixed stop 1.5 ATR.\n\nGross P&L INR {}; trades {}; wins {}; closed-trade drawdown INR {}. Costs are excluded.\n\n| Date (IST) | Bars | Trades | Gross P&L INR |\n|---|---:|---:|---:|\n",
        stats["gross_pnl_inr"],
        stats["trades"],
        stats["wins"],
        stats["max_closed_trade_drawdown_inr"]
    );
    for row in &daily {
        md.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            row["date_ist"].as_str().unwrap(),
            row["bars"],
            row["statistics"]["trades"],
            row["statistics"]["gross_pnl_inr"]
        ));
    }
    md.push_str("\nEntries use completed-bar confirmations and the next open. Stops are simulated by the native OHLC matching engine. VWAP resets each IST session. Positions are flat daily. No fees, spread, slippage or liquidity constraints are modeled. Drawdown is measured only on closed trades. Raw native annualized statistics include warmup and are not strategy-quality estimates.\n\nEach date directory contains summary, candles, indicator values, signals, fills and trades. historical_input.json permits offline reproduction. Real orders remain disabled.\n");
    std::fs::write(folder.join("README.md"), md)?;
    Ok(summary)
}

//! Closed-trade accounting, independently cross-checked against native P&L.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
pub fn trades(fills: &[Value]) -> Result<Vec<Value>> {
    ensure!(fills.len().is_multiple_of(2), "Unpaired fills");
    let mut trades = Vec::new();
    for pair in fills.as_chunks::<2>().0 {
        let a = &pair[0];
        let b = &pair[1];
        ensure!(
            a["reason"] == "entry" && b["reason"] != "entry" && a["side"] != b["side"],
            "Invalid entry/exit sequence"
        );
        ensure!(
            a["quantity"].as_str() == Some("1") && b["quantity"].as_str() == Some("1"),
            "Unexpected fill quantity"
        );
        let entry = a["price"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing price"))?
            .parse::<f64>()?;
        let exit = b["price"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing price"))?
            .parse::<f64>()?;
        let pnl = (exit - entry) * 100. * if a["side"] == "BUY" { 1. } else { -1. };
        trades.push(json!({"entry_ns":a["timestamp_ns"],"exit_ns":b["timestamp_ns"],"direction":if a["side"]=="BUY" {"LONG"} else {"SHORT"},"entry_price":entry,"exit_price":exit,"exit_reason":b["reason"],"gross_pnl_inr":pnl}));
    }
    Ok(trades)
}
pub fn statistics(trades: &[Value]) -> Value {
    let mut equity = 0_f64;
    let mut peak = 0_f64;
    let mut drawdown = 0_f64;
    let mut wins = 0;
    let mut gains = 0_f64;
    let mut losses = 0_f64;
    for t in trades {
        let p = t["gross_pnl_inr"].as_f64().expect("validated trade");
        equity += p;
        peak = peak.max(equity);
        drawdown = drawdown.max(peak - equity);
        if p > 0. {
            wins += 1;
            gains += p;
        } else {
            losses -= p;
        }
    }
    json!({"trades":trades.len(),"wins":wins,"gross_pnl_inr":equity,
        "win_rate":if trades.is_empty(){None}else{Some(wins as f64/trades.len() as f64)},
        "profit_factor":if losses>0.{Some(gains/losses)}else{None},
        "max_closed_trade_drawdown_inr":drawdown,"ending_equity_inr":1_000_000.+equity})
}

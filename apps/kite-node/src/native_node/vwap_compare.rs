//! Reproducible entry-filter comparison on one immutable historical input.
use super::{
    backtest_report as report, supertrend_input, vwap_batch, vwap_filters::Variant, vwap_input,
};
use anyhow::Result;
use chrono::NaiveDate;
use nautilus_core::UUID4;
use serde_json::json;
pub fn run(end: &str, path: &str) -> Result<()> {
    let end = NaiveDate::parse_from_str(end, "%Y-%m-%d")?;
    let days = vwap_input::dates(end)?;
    execute(end, path, &days, false)
}
pub fn run_range(start: &str, end: &str, path: &str) -> Result<()> {
    let start = NaiveDate::parse_from_str(start, "%Y-%m-%d")?;
    let end = NaiveDate::parse_from_str(end, "%Y-%m-%d")?;
    let days = vwap_input::range(start, end)?;
    execute(end, path, &days, true)
}
fn execute(end: NaiveDate, path: &str, days: &[NaiveDate], fetch_warmup: bool) -> Result<()> {
    let folder = report::directory(
        &format!("vwap_filter_comparison_{}_to_{end}", days[0]),
        &UUID4::new().to_string(),
    )?;
    println!("Comparison results: {}", folder.display());
    let result = (|| {
        // Validate the file before copying; every variant reads the same snapshot.
        let mut input = supertrend_input::load(end, Some(path))?;
        let snapshot = folder.join("historical_input.json");
        let warmup = input
            .candles
            .iter()
            .filter(|c| c.time().is_ok_and(|t| t.date_naive() < days[0]))
            .count();
        if fetch_warmup && warmup < 100 {
            let prior = days[0]
                .pred_opt()
                .ok_or_else(|| anyhow::anyhow!("Date overflow"))?;
            let mut earlier = tokio::runtime::Runtime::new()?.block_on(
                kite_adapter::http::historical::fetch_window(input.instrument_token, prior, 7),
            )?;
            earlier.retain(|c| c.time().is_ok_and(|t| t.date_naive() < days[0]));
            earlier.extend(input.candles);
            input.candles = earlier;
            kite_adapter::http::historical::validate(&input.candles)?;
            report::json(&folder, "historical_input.json", &input)?;
        } else {
            std::fs::copy(path, &snapshot)?;
        }
        let mut rows = Vec::new();
        for variant in [Variant::Baseline, Variant::Trend, Variant::Breakout] {
            let run_folder = folder.join(variant.name());
            std::fs::create_dir(&run_folder)?;
            let result = vwap_batch::execute(
                end,
                Some(
                    snapshot
                        .to_str()
                        .ok_or_else(|| anyhow::anyhow!("Invalid path"))?,
                ),
                days,
                &run_folder,
                variant,
            )?;
            let trades: Vec<serde_json::Value> =
                serde_json::from_slice(&std::fs::read(run_folder.join("trades.json"))?)?;
            let last_day: Vec<_> = trades
                .iter()
                .filter(|t| t["date_ist"] == end.to_string())
                .collect();
            rows.push(json!({"variant":variant.name(),"statistics":result["statistics"],"sessions":result["sessions"],"last_day_trades":last_day}));
        }
        let output = json!({"status":"completed","end_date_ist":end.to_string(),"start_date_ist":days[0].to_string(),"calendar_days":(end-days[0]).num_days()+1,"sessions":days.len(),"variants":rows,
            "instrument":"CRUDEOIL26SEPFUT.MCX","interval":"5minute","contracts":1,
            "fixed_stop":"1.5 ATR(14) from actual entry; ATR at entry-signal close; no trailing",
            "unchanged_exits":"original fully confirmed opposite setup, fixed stop or session close",
            "trend_rule":"baseline entry plus strict MACD zero-line direction and both EMA slopes over one bar",
            "breakout_rule":"baseline qualifying crossover arms setup; subsequent close beyond setup high/low within next 3 bars; EMA order, VWAP and MACD confirmation retained; cancel on lost confirmation, expiry or new session; no zero-line/slope filter",
            "costs":"fees, spread and slippage excluded","selection_warning":"expanded historical comparison includes previously inspected dates; not independent forward validation",
            "default_strategy":"baseline unchanged","live_orders_enabled":false});
        report::json(&folder, "comparison.json", &output)?;
        let mut md = format!(
            "# VWAP entry-filter comparison\n\nSame saved {} sessions ({} through {}), five-minute CRUDEOIL candles, one lot and fixed 1.5 ATR(14) stops. Original opposite-setup exits and end-of-day exits are identical across variants.\n\n| Entry variant | Trades | Wins | Gross P&L INR | Closed-trade drawdown INR |\n|---|---:|---:|---:|---:|\n",
            days.len(),
            days[0],
            end
        );
        for row in &rows {
            let s = &row["statistics"];
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                row["variant"].as_str().unwrap(),
                s["trades"],
                s["wins"],
                s["gross_pnl_inr"],
                s["max_closed_trade_drawdown_inr"]
            ));
        }
        md.push_str("\nBaseline: original entry conditions. Trend: adds MACD zero-line and both EMA slopes in trade direction. Breakout: waits up to three subsequent closes beyond the qualifying crossover candle high/low while EMA order, VWAP and MACD confirmation remain valid; resets at session change. A wick is insufficient. Breakout is tested alone, not combined with trend filters.\n\nEvery entry executes at the next open. ATR uses the completed entry-signal candle. Costs are excluded; drawdown uses closed trades only. This same-sample comparison does not establish future performance. The default baseline is unchanged and real orders remain disabled.\n\nSee comparison.json and each variant's daily reports, indicator traces, fills and trades.\n");
        std::fs::write(folder.join("README.md"), md)?;
        Ok::<_, anyhow::Error>(output)
    })();
    if let Err(error) = &result {
        report::json(
            &folder,
            "failure.json",
            &json!({"status":"failed","error":error.to_string()}),
        )?;
    }
    result.map(|output| println!("{output}"))
}

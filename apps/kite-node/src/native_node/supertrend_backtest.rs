//! Isolated native BacktestNode entry point; never constructs a broker execution client.
use super::{
    backtest_report as report, persistence,
    supertrend_actor::{BarStrategy, State},
    supertrend_input as input,
};
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use nautilus_backtest::{
    config::{BacktestEngineConfig, BacktestRunConfig, BacktestVenueConfig},
    node::BacktestNode,
};
use nautilus_common::{cache::database::CacheDatabaseFactory, logging::logger::LoggerConfig};
use nautilus_core::UUID4;
use nautilus_model::{
    data::BarType,
    enums::{AccountType, BookType, OmsType},
    instruments::InstrumentAny,
};
use std::{cell::RefCell, path::Path, rc::Rc};
pub fn run(date: &str, path: Option<&str>) -> Result<()> {
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
    let instance = UUID4::new();
    let folder = report::directory(
        &format!("supertrend_7_2_5minute_{date}"),
        &instance.to_string(),
    )?;
    println!("Backtest results: {}", folder.display());
    let result = (|| {
        let data = input::load(date, path)?;
        execute(date, data, instance, &folder)
    })();
    if let Err(error) = &result {
        report::json(
            &folder,
            "failure.json",
            &serde_json::json!({"status":"failed","date":date.to_string(),"error":error.to_string(),"live_orders_enabled":false,"pnl_report_available":false}),
        )?;
    }
    result.map(|value| println!("{value}"))
}
fn execute(
    date: NaiveDate,
    data: input::Input,
    instance: UUID4,
    folder: &Path,
) -> Result<serde_json::Value> {
    execute_variant(date, data, instance, folder, false)
}
pub fn execute_variant(
    date: NaiveDate,
    data: input::Input,
    instance: UUID4,
    folder: &Path,
    filtered: bool,
) -> Result<serde_json::Value> {
    let count = input::validate(&data, date)?;
    let (instrument, _) = crate::paper_flow::simulation::fixture()?;
    let minutes = super::vwap_input::step_ns(&data.interval)? / 60_000_000_000;
    let bars: BarType = format!("CRUDEOIL26SEPFUT.MCX-{minutes}-MINUTE-LAST-EXTERNAL").parse()?;
    let replay = input::replay(&data, date, bars)?;
    report::json(folder, "candles.json", &data)?;
    let run_id = instance.to_string();
    let cache_config = persistence::cache_config();
    let engine_config = BacktestEngineConfig {
        trader_id: "SUSANTA-001".into(),
        instance_id: Some(instance),
        cache: Some(cache_config.clone()),
        save_state: true,
        load_state: false,
        shutdown_on_error: true,
        logging: LoggerConfig {
            stdout_level: log::LevelFilter::Warn,
            is_colored: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let venue = BacktestVenueConfig::builder()
        .name("MCX")
        .oms_type(OmsType::Netting)
        .account_type(AccountType::Margin)
        .book_type(BookType::L1_MBP)
        .starting_balances(vec!["1000000 INR".into()])
        .build()?;
    let config = BacktestRunConfig::builder()
        .id(run_id.clone())
        .engine(engine_config)
        .venues(vec![venue])
        .data(vec![])
        .raise_exception(true)
        .dispose_on_completion(false)
        .build()?;
    let mut node = BacktestNode::new(vec![config])?;
    node.build()?;
    let state = Rc::new(RefCell::new(State::default()));
    let (start, end) = super::vwap_input::bounds(date)?;
    let engine = node.get_engine_mut(&run_id).expect("built node");
    let db = nautilus_common::live::get_runtime().block_on(
        super::redis_cache::Factory(persistence::redis_config()?).create(
            "SUSANTA-001".into(),
            instance,
            cache_config,
        ),
    )?;
    engine.kernel_mut().cache.borrow_mut().set_database(db);
    engine.add_instrument(&InstrumentAny::FuturesContract(instrument))?;
    engine.add_strategy(
        BarStrategy::new(bars, start, end, state.clone()).with_confirmation(filtered),
    )?;
    engine.add_data(replay, None, false, false)?;
    let results = node.run()?;
    let cache = node
        .get_engine_mut(&run_id)
        .expect("engine")
        .kernel()
        .cache
        .borrow();
    let open: f64 = cache
        .positions_open(None, None, None, None, None)
        .iter()
        .map(|p| p.signed_qty)
        .sum();
    let pending = cache.orders_open(None, None, None, None, None).len();
    drop(cache);
    let s = state.borrow();
    ensure!(
        s.fills.len() == s.signals.len(),
        "Missing or unmatched fills"
    );
    let mut fills = s.fills.clone();
    for (fill, signal) in fills.iter_mut().zip(&s.signals) {
        ensure!(
            fill["timestamp_ns"] == signal["timestamp_ns"],
            "Fill not at intended next open"
        );
        fill["reason"] =
            serde_json::json!(if signal["intent"] == "BUY" || signal["intent"] == "SELL" {
                "entry"
            } else if signal["reason"] == "end_of_day" {
                "end_of_day"
            } else {
                "supertrend_reversal"
            });
    }
    let trades = super::vwap_report::trades(&fills)?;
    let stats = super::vwap_report::statistics(&trades);
    let native = serde_json::to_value(&results)?;
    let pnl = native[0]["stats_pnls"]["INR"]["PnL (total)"]
        .as_f64()
        .unwrap_or(0.);
    ensure!(
        (pnl - stats["gross_pnl_inr"].as_f64().unwrap()).abs() < 0.01,
        "Native P&L mismatch"
    );
    report::json(folder, "trades.json", &trades)?;
    report::json(folder, "fills.json", &fills)?;
    report::json(folder, "signals.json", &s.signals)?;
    report::json(folder, "indicators.json", &s.indicators)?;
    let output = serde_json::json!({
        "event":"native_supertrend_backtest_complete","status":"completed","namespace":run_id,
        "date_ist":date.to_string(),"instrument":data.instrument_id,"data_source":data.source,
        "interval":data.interval,"atr_period":7,"atr_smoothing":"Nautilus Wilder (first true-range seed)",
        "multiplier":2,"warmup_bars":data.candles.len()-count,"session_bars":count,
        "entry_confirmation":filtered,"statistics":stats,
        "execution_model":"completed-bar signal, next-open synthetic zero-spread quote; separate exit then entry",
        "session_ist":if (end-start)==6*3_600_000_000_000+30*60_000_000_000 {"17:00–23:30"} else {"09:00–23:30"},"position_size_contracts":1,"contract_multiplier":100,
        "end_of_day":"forced simulated exit at last candle close",
        "fees":"excluded","slippage":"excluded","pnl_basis":"gross before fees, spread and slippage",
        "stop_loss_target":"not enabled; exit on Supertrend reversal or session close",
        "native_backtest_node":true,"native_atr":true,"native_redis_cache":true,
        "signal_count":s.signals.len(),"fills":s.fills.len(),"open_contracts":open,
        "errors":s.errors,"results":results,"result_directory":folder,
        "live_orders_enabled":false,"broker_orders_sent":false
    });
    let valid = s.started
        && s.stopped
        && s.errors.is_empty()
        && open == 0.
        && pending == 0
        && results.len() == 1;
    drop(s);
    node.get_engine_mut(&run_id).expect("engine").dispose();
    ensure!(
        valid,
        "Native Supertrend lifecycle/fill/flat checks failed; inspect saved diagnostics"
    );
    report::json(folder, "summary.json", &output)?;
    let pnl = &output["results"][0]["stats_pnls"]["INR"]["PnL (total)"];
    let text = format!(
        "# Supertrend backtest — {date}\n\n- CRUDEOIL26SEPFUT.MCX, {minutes}-minute candles, ATR(7) Wilder, multiplier 2.\n- Data: {}. Warmup bars: {}. Session bars: {count}. Entry confirmations: {filtered}.\n- Simulated fills: {}. Open contracts: {}.\n- Gross P&L (INR): {}. Fees, spread and slippage excluded.\n- Signals use completed bars; executions use the next open. End-of-day exit uses the last close.\n- No extra stop-loss/target overlay. No real orders sent.\n\nSee summary.json for Nautilus statistics, fills.json, signals.json, indicators.json and candles.json for the audit trail.\n",
        data.source,
        data.candles.len() - count,
        output["fills"],
        open,
        pnl
    );
    std::fs::write(folder.join("README.md"), text)?;
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_supertrend_backtest_roundtrip_long_short_flat_and_next_open() {
        // Nautilus has process-global runtime endpoints; isolate this engine from other unit tests.
        if std::env::var_os("KITE_SUPERTREND_TEST_CHILD").is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "native_node::supertrend_backtest::tests::native_supertrend_backtest_roundtrip_long_short_flat_and_next_open", "--nocapture"])
                .env("KITE_SUPERTREND_TEST_CHILD", "1").output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let folder = tempfile::tempdir().unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 9, 15).unwrap();
        let result = execute(date, input::fixture(), UUID4::new(), folder.path()).unwrap();
        assert_eq!(result["open_contracts"], 0.);
        assert!(result["fills"].as_u64().unwrap() > 4);
        let signals: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(folder.path().join("signals.json")).unwrap())
                .unwrap();
        for intent in ["BUY", "BUY_EXIT", "SELL", "SELL_EXIT"] {
            assert!(
                signals.iter().any(|s| s["intent"] == intent),
                "missing {intent}"
            );
        }
        let fills: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(folder.path().join("fills.json")).unwrap())
                .unwrap();
        for f in fills {
            let ts = f["timestamp_ns"].as_u64().unwrap();
            let (start, end) = input::bounds(date).unwrap();
            assert!(ts >= start && ts <= end + 2);
            let expected = if ts >= end {
                input::fixture().candles.last().unwrap().close
            } else {
                input::fixture().candles[174 + ((ts - start) / 300_000_000_000) as usize].open
            };
            assert_eq!(
                f["price"].as_str().unwrap().parse::<f64>().unwrap(),
                expected
            );
        }
        assert_eq!(signals.last().unwrap()["reason"], "end_of_day");
        assert!(folder.path().join("summary.json").exists());
    }
}

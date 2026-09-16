//! Native confirmed Supertrend session with fixed ATR protection; no broker client.

use super::{
    backtest_report as report, persistence, supertrend_actor::State, supertrend_input::Input,
    supertrend_stop_actor::StopStrategy, vwap_input as input, vwap_report,
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
pub fn run(date: &str, path: &str, folder: &str, key: &str) -> Result<()> {
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
    let mut con = redis::Client::open(kite_journal::connection::url_from_env()?.as_str())?
        .get_connection()?;
    let blocked: i8 = redis::cmd("GET").arg(key).query(&mut con)?;
    ensure!((-1..=1).contains(&blocked), "Invalid cooldown direction");
    let folder = Path::new(folder);
    std::fs::create_dir(folder)?;
    let data = super::supertrend_input::load(date, Some(path))?;
    let output = execute(date, data, UUID4::new(), folder, blocked)?;
    redis::cmd("SET")
        .arg(key)
        .arg(output["blocked_direction"].as_i64().unwrap())
        .query::<()>(&mut con)?;
    println!("{output}");
    Ok(())
}
pub fn execute(
    date: NaiveDate,
    data: Input,
    instance: UUID4,
    folder: &Path,
    blocked: i8,
) -> Result<serde_json::Value> {
    let count = input::validate(&data, date)?;
    let (instrument, _) = crate::paper_flow::simulation::fixture()?;
    let bt: BarType = "CRUDEOIL26SEPFUT.MCX-5-MINUTE-LAST-EXTERNAL".parse()?;
    let replay = input::replay(&data, date, bt)?;
    report::json(folder, "candles.json", &data)?;
    let run_id = instance.to_string();
    let cache_config = persistence::cache_config();
    let config = BacktestEngineConfig {
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
        .bar_execution(true)
        .starting_balances(vec!["1000000 INR".into()])
        .build()?;
    let run = BacktestRunConfig::builder()
        .id(run_id.clone())
        .engine(config)
        .venues(vec![venue])
        .data(vec![])
        .raise_exception(true)
        .dispose_on_completion(false)
        .build()?;
    let mut node = BacktestNode::new(vec![run])?;
    node.build()?;
    let state = Rc::new(RefCell::new(State::default()));
    let (start, end) = input::bounds(date)?;
    let engine = node.get_engine_mut(&run_id).expect("built engine");
    let db = nautilus_common::live::get_runtime().block_on(
        super::redis_cache::Factory(persistence::redis_config()?).create(
            "SUSANTA-001".into(),
            instance,
            cache_config,
        ),
    )?;
    engine.kernel_mut().cache.borrow_mut().set_database(db);
    engine.add_instrument(&InstrumentAny::FuturesContract(instrument))?;
    engine.add_strategy(StopStrategy::new(bt, start, end, state.clone(), blocked))?;
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
    report::json(folder, "fills.json", &s.fills)?;
    report::json(folder, "signals.json", &s.signals)?;
    report::json(folder, "indicators.json", &s.indicators)?;
    let valid = s.started
        && s.stopped
        && s.errors.is_empty()
        && open == 0.
        && pending == 0
        && results.len() == 1;
    let fills = s.fills.clone();
    let errors = s.errors.clone();
    let blocked_direction = s.blocked_direction;
    drop(s);
    node.get_engine_mut(&run_id).expect("engine").dispose();
    ensure!(
        valid,
        "Native session validation failed: {errors:?}; open={open}, pending={pending}"
    );
    let trades = vwap_report::trades(&fills)?;
    let stats = vwap_report::statistics(&trades);
    let native = serde_json::to_value(&results)?;
    let native_pnl = native[0]["stats_pnls"]["INR"]["PnL (total)"]
        .as_f64()
        .unwrap_or(0.);
    ensure!(
        (native_pnl - stats["gross_pnl_inr"].as_f64().unwrap()).abs() < 0.01,
        "Native P&L differs from fill-pair arithmetic"
    );
    report::json(folder, "trades.json", &trades)?;
    let output = serde_json::json!({"status":"completed","event":"native_supertrend_stop_backtest_complete","namespace":run_id,
        "entry_variant":"confirmed_atr_stop","blocked_direction":blocked_direction,
        "date_ist":date.to_string(),"instrument":data.instrument_id,"data_source":data.source,"interval":"5minute",
        "supertrend":[7,2],"macd":[12,26,9],"atr_period":14,"stop_atr_multiplier":1.5,"stop_type":"fixed native simulated stop-market, rounded outward to whole rupee",
        "vwap":"session HLC3 volume weighted, reset IST day","session_bars":count,"warmup_bars":data.candles.len()-count,
        "entry_model":"Supertrend direction plus MACD vs signal and price vs VWAP, completed bar then next open; after stop wait for fresh Supertrend change",
        "exit_model":"stop-loss, Supertrend reversal, or end-of-day",
        "fees":"excluded","spread":"excluded","slippage":"excluded","stop_fill_model":"native OHLC matching",
        "native_indicators":true,"native_backtest_node":true,"native_redis_cache":true,
        "open_contracts":open,"fills":fills.len(),"statistics":stats,"results":results,
        "live_orders_enabled":false,"broker_orders_sent":false});
    report::json(folder, "summary.json", &output)?;
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_supertrend_gap_stop_uses_open() {
        if std::env::var_os("KITE_ST_STOP_GAP_CHILD").is_none() {
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "native_node::supertrend_stop_backtest::tests::native_supertrend_gap_stop_uses_open",
                    "--nocapture",
                ])
                .env("KITE_ST_STOP_GAP_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let date = NaiveDate::from_ymd_opt(2026, 9, 15).unwrap();
        let mut data = super::super::supertrend_input::fixture();
        let mut policy = super::super::supertrend_stop_policy::Policy::new(0, 0);
        let mut setup = None;
        for (i, c) in data.candles.iter().enumerate() {
            let time = c.time().unwrap();
            let r = policy.update(
                c.high,
                c.low,
                c.close,
                c.volume as f64,
                time.timestamp_nanos_opt().unwrap() as u64 + 300_000_000_000,
            );
            if time.date_naive() == date && r.entry != 0 {
                setup = Some((i, r.entry, r.atr));
                break;
            }
        }
        let (i, direction, atr) = setup.expect("fixture has confirmed setup");
        let entry = &mut data.candles[i + 1];
        let stop = super::super::vwap_signal::stop_price(entry.open, atr, direction == 1);
        entry.high = entry.open + 1.;
        entry.low = entry.open - 1.;
        entry.close = entry.open;
        let gap = &mut data.candles[i + 2];
        let gap_price = stop - if direction == 1 { 20. } else { -20. };
        gap.open = gap_price;
        gap.high = gap_price + 1.;
        gap.low = gap_price - 1.;
        gap.close = gap_price;
        let folder = tempfile::tempdir().unwrap();
        execute(date, data, UUID4::new(), folder.path(), 0).unwrap();
        let signals: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(folder.path().join("signals.json")).unwrap())
                .unwrap();
        let first_stop = signals
            .iter()
            .find(|s| s["intent"] == "PLACE_STOP")
            .unwrap();
        assert_eq!(first_stop["stop_price"].as_f64().unwrap(), stop);
        let fills: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(folder.path().join("fills.json")).unwrap())
                .unwrap();
        let exit = fills
            .iter()
            .find(|f| f["client_order_id"] == first_stop["client_order_id"])
            .unwrap();
        assert_eq!(
            exit["price"].as_str().unwrap().parse::<f64>().unwrap(),
            gap_price,
            "A gapped stop must not get the better trigger price"
        );
    }
    #[test]
    fn native_supertrend_stops_and_flatness() {
        if std::env::var_os("KITE_ST_STOP_TEST_CHILD").is_none() {
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "native_node::supertrend_stop_backtest::tests::native_supertrend_stops_and_flatness",
                    "--nocapture",
                ])
                .env("KITE_ST_STOP_TEST_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let folder = tempfile::tempdir().unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 9, 15).unwrap();
        let mut data = super::super::supertrend_input::fixture();
        let mut policy = super::super::supertrend_stop_policy::Policy::new(0, 0);
        let mut last = 0;
        for c in &mut data.candles {
            if c.time().unwrap().date_naive() == date {
                if last == 1 {
                    c.low = c.open - 1000.;
                } else if last == -1 {
                    c.high = c.open + 1000.;
                }
            }
            let ts = c.time().unwrap().timestamp_nanos_opt().unwrap() as u64 + 300_000_000_000;
            last = policy
                .update(c.high, c.low, c.close, c.volume as f64, ts)
                .entry;
        }
        let out = execute(date, data, UUID4::new(), folder.path(), 0).unwrap();
        assert_eq!(out["open_contracts"], 0.);
        let fills: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(folder.path().join("fills.json")).unwrap())
                .unwrap();
        assert!(
            fills.iter().any(|f| f["reason"] == "stop_loss"),
            "No native stop fill"
        );
        for intent in ["BUY", "BUY_EXIT", "SELL", "SELL_EXIT"] {
            assert!(
                fills.iter().any(|f| f["intent"] == intent),
                "Missing {intent}"
            );
        }
        let signals: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(folder.path().join("signals.json")).unwrap())
                .unwrap();
        for f in fills.iter().filter(|f| f["reason"] == "stop_loss") {
            let stop = signals
                .iter()
                .find(|s| s["client_order_id"] == f["client_order_id"])
                .unwrap();
            assert_eq!(
                f["price"].as_str().unwrap().parse::<f64>().unwrap(),
                stop["stop_price"].as_f64().unwrap()
            );
        }
    }
}

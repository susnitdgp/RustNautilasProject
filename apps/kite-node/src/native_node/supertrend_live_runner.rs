//! Selected five-minute strategy in LiveNode with Sandbox execution only.
use super::{
    data, persistence,
    supertrend_actor::{BarStrategy, State},
    supertrend_live_bars as bars,
    supertrend_live_control::Control,
    supertrend_live_data as feed,
    supertrend_live_lease::Lease,
};
use anyhow::{Result, ensure};
use kite_adapter::http::historical::Candle;
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_core::UUID4;
use nautilus_live::config::LiveNodeConfig;
use nautilus_model::{
    data::BarType,
    types::{Currency, Money},
};
use nautilus_sandbox::{
    config::SandboxExecutionClientConfig, factory::SandboxExecutionClientFactory,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
fn synthetic() -> Result<(Vec<Candle>, Vec<Candle>)> {
    let first = data::now() / 300_000_000_000 * 300_000_000_000 - 160 * 300_000_000_000;
    let mut candles = Vec::new();
    for i in 0..160u64 {
        let x = i % 40;
        let p = 6000.
            + if x < 20 {
                x as f64 * 10.
            } else {
                (40 - x) as f64 * 10.
            };
        let timestamp =
            chrono::DateTime::from_timestamp_nanos((first + i * 300_000_000_000) as i64)
                .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
                .to_rfc3339();
        candles.push(Candle {
            timestamp,
            open: p,
            high: p + 5.,
            low: p - 5.,
            close: p + 1.,
            volume: 100,
            oi: 1000,
        });
    }
    let live = candles.split_off(120);
    Ok((candles, live))
}
pub fn run(config: &str, seconds: u64, sim: bool) -> Result<()> {
    let _ = super::production::Selection::load(config)?;
    ensure!(
        (5..=290).contains(&seconds),
        "Paper duration must be 5..290 seconds"
    );
    let date = chrono::Utc::now()
        .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
        .date_naive();
    let (instrument, token, credentials) = if sim {
        (crate::paper_flow::simulation::fixture()?.0, 144870151, None)
    } else {
        let c = crate::preflight_command::read_config("config/crudeoil-september.toml")?;
        let report = crate::preflight_command::resolve(&c, None)?;
        (
            kite_adapter::instruments::contract::build(&report, data::now().into())?,
            report.instrument_token,
            Some(Arc::new(kite_adapter::credentials::redis::load_from_env()?)),
        )
    };
    let (warmup, simulated) = if sim {
        synthetic()?
    } else {
        let raw = tokio::runtime::Runtime::new()?
            .block_on(kite_adapter::http::historical::fetch_window(token, date, 7))?;
        (bars::completed(raw, data::now())?, Vec::new())
    };
    let _ = bars::Tracker::new(&warmup)?;
    if !sim {
        bars::validate_warmup(&warmup, date, data::now())?;
    }
    let (start, end) = if sim {
        (bars::close(warmup.last().unwrap())?, u64::MAX)
    } else {
        let (session_start, end) = super::vwap_input::bounds(date)?;
        ensure!(
            data::now() >= session_start && data::now() + 15_000_000_000 < end,
            "Start paper mode during the configured session"
        );
        (data::now(), end)
    };
    let redis = persistence::redis_config()?;
    let id = UUID4::new();
    let control = Control::new(sim);
    let state = Rc::new(RefCell::new(State::default()));
    let mut lease = Lease::acquire(&id.to_string(), sim)?;
    let feed_config = feed::Config {
        instrument: instrument.clone(),
        token,
        date,
        warmup: warmup.clone(),
        simulated,
        control: control.clone(),
    };
    let outcome=tokio::runtime::Runtime::new()?.block_on(async {
        let mut cfg=LiveNodeConfig{environment:Environment::Sandbox,trader_id:"SUSANTA-001".into(),instance_id:Some(id),
            cache:Some(persistence::cache_config()),save_state:true,load_state:false,shutdown_on_error:true,
            delay_post_stop:Duration::from_secs(2),..Default::default()};
        cfg.logging=LoggerConfig{stdout_level:log::LevelFilter::Warn,is_colored:false,..Default::default()};
        cfg.exec_engine.reconciliation=false;
        cfg.risk_engine.max_notional_per_order.insert(instrument.id.to_string(),"2000000".into());
        let mut builder=nautilus_live::builder::LiveNodeBuilder::from_config(cfg)?
            .with_cache_database_factory(Box::new(super::redis_cache::Factory(redis)))
            .add_data_client(Some("STBARS".into()),Box::new(feed::Factory),Box::new(feed_config))?;
        if !sim {
            builder=builder.add_data_client(Some("KITE".into()),Box::new(data::Factory),Box::new(data::Config{
                instrument:instrument.clone(),token,seconds:seconds+10,synthetic_tick_ms:500,short_fixture:false,sandbox_user:None,credentials}))?;
        }
        let simulation=SandboxExecutionClientConfig{venue:"MCX".into(),account_id:"MCX-PAPER".into(),base_currency:Some(Currency::INR()),
            bar_execution:false,starting_balances:vec![Money::new(1_000_000.,Currency::INR())],..Default::default()};
        let mut node=builder.add_simulated_exec_client(Some("MCX".into()),Box::new(SandboxExecutionClientFactory::new()),Box::new(simulation))?.build()?;
        let bt:BarType=format!("{}-5-MINUTE-LAST-EXTERNAL",instrument.id).parse()?;
        node.add_strategy(BarStrategy::new(bt,start,end,state.clone()).with_confirmation(true).with_live(control.clone()))?;
        let handle=node.handle();let ctl=control.clone();
        let watcher=tokio::spawn(async move {
            tokio::select!{_=super::lifecycle::wait(ctl.done.clone(),seconds)=>{},_=tokio::time::sleep(Duration::from_secs(seconds))=>{}}
            ctl.stop();
            tokio::time::sleep(Duration::from_millis(250)).await;
            for _ in 0..50 {
                if ctl.flat.load(Ordering::Acquire){break;}
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            handle.stop();
        });
        println!("{}",serde_json::json!({"event":"supertrend_live_started","namespace":id.to_string(),"runtime":"LiveNode","strategy":"supertrend_macd_vwap","interval":"5minute","simulated_feed":sim,"execution":"Nautilus Sandbox","warmup_bars":warmup.len(),"live_orders_enabled":false}));
        let result=node.run_with_mode(nautilus_live::node::NodeRunMode::Hosted).await;watcher.abort();
        let position=state.borrow().cache.as_ref().map(|cache|cache.borrow().positions_open(None,None,None,None,None).iter().map(|p|p.signed_qty).sum::<f64>()).unwrap_or(0.);
        let pending=state.borrow().cache.as_ref().map(|cache|cache.borrow().orders_open(None,None,None,None,None).len()+cache.borrow().orders_inflight(None,None,None,None,None).len()).unwrap_or(0);
        node.dispose();result?;Ok::<_,anyhow::Error>((position,pending))
    });
    let fault = control.fault.lock().expect("fault lock").clone();
    let (position, pending) = outcome.as_ref().copied().unwrap_or((f64::NAN, 1));
    let clean = outcome.is_ok()
        && position == 0.
        && pending == 0
        && fault.is_none()
        && state.borrow().errors.is_empty()
        && state.borrow().stopped;
    lease.finish(clean, position)?;
    let folder = std::path::PathBuf::from(format!("data/supertrend-live/{id}"));
    std::fs::create_dir_all(&folder)?;
    let s = state.borrow();
    super::backtest_report::json(&folder, "indicators.json", &s.indicators)?;
    super::backtest_report::json(&folder, "signals.json", &s.signals)?;
    super::backtest_report::json(&folder, "fills.json", &s.fills)?;
    let output = serde_json::json!({"event":"supertrend_live_complete","namespace":id.to_string(),"status":if clean{"Clean"}else{"ReviewRequired"},
        "runtime":"LiveNode","strategy":"supertrend_macd_vwap","interval":"5minute","contracts":1,"atr_stop_enabled":false,
        "simulated_feed":sim,"execution":"Nautilus Sandbox","quotes":s.live_quotes,"rejected_quotes":s.rejected_quotes,
        "bars":s.indicators.len(),"warmup_bars":warmup.len(),"signals":s.signals.len(),"fills":s.fills.len(),"open_contracts":position,
        "open_orders":pending,"errors":s.errors,"feed_fault":fault,"run_error":outcome.as_ref().err().map(ToString::to_string),
        "automatic_resume_enabled":false,"report_directory":folder,"live_orders_enabled":false,"broker_orders_sent":false});
    super::backtest_report::json(&folder, "summary.json", &output)?;
    println!("{output}");
    outcome?;
    ensure!(clean, "Paper run requires recovery review");
    Ok(())
}

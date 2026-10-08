//! Nautilus LiveNode harness for ILRC with the *native Kite mock* execution client.
//! Live Kite production registration is disabled pending full broker stop/exit recovery tests.
use super::{
    data, ilrc_config::Selection, ilrc_live_actor::IlrcActor, live_bars, live_control::Control,
    live_data, persistence,
};
use anyhow::{Result, ensure};
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_core::UUID4;
use nautilus_live::{builder::LiveNodeBuilder, config::LiveNodeConfig, node::NodeRunMode};
use nautilus_model::data::BarType;
use std::{
    sync::{Arc, atomic::AtomicI64},
    time::Duration,
};

pub fn run_mock(config: &str, seconds: u64) -> Result<()> {
    ensure!(
        (5..=86360).contains(&seconds),
        "ILRC mock duration out of range"
    );
    let selection = Selection::load(config)?;
    let now = chrono::Utc::now().with_timezone(&chrono::FixedOffset::east_opt(19800).expect("IST"));
    let date = now.date_naive();
    ensure!(
        selection.session_calendar.session(date)?.is_some(),
        "ILRC selected date is not a trading session"
    );
    let master = kite_adapter::http::instruments::download()?;
    let report = kite_adapter::preflight::run_selected(
        &selection.symbol,
        selection.instrument_token,
        &master[..],
        date,
    )?;
    ensure!(
        report.instrument_id == selection.instrument
            && report.expiry == selection.expected_expiry.to_string(),
        "Live Kite instrument master does not match selected contract"
    );
    let instrument = kite_adapter::instruments::contract::build(&report, data::now().into())?;
    let raw = tokio::runtime::Runtime::new()?.block_on(
        kite_adapter::http::historical::fetch_window_for(
            selection.instrument_token,
            date,
            8,
            selection.interval,
        ),
    )?;
    let warmup = live_bars::broker_finalized_for(raw, data::now(), selection.interval)?;
    ensure!(warmup.len() >= 110, "ILRC history warmup incomplete");
    live_bars::validate_broker_finalized_warmup_for(
        &warmup,
        date,
        data::now(),
        &selection.session_calendar,
        selection.interval,
    )?;
    let run_id = UUID4::new();
    let market_price = Arc::new(AtomicI64::new(0));
    let control = Control::new(false).with_bar_ns(selection.interval.nanoseconds());
    let credentials = Arc::new(kite_adapter::credentials::redis::load_from_env()?);
    let feed = live_data::Config {
        instrument: instrument.clone(),
        token: selection.instrument_token,
        date,
        calendar: selection.session_calendar.clone(),
        interval: selection.interval,
        volume_sensitive: false,
        synthetic_delay_ms: 60,
        warmup,
        simulated: Vec::new(),
        control: control.clone(),
        emit_intrabar_ticks: false,
    };
    let redis = persistence::redis_config()?;
    let account_id = format!(
        "ILRC{}",
        run_id
            .to_string()
            .replace("-", "")
            .chars()
            .take(14)
            .collect::<String>()
    );
    kite_adapter::execution::native_client::coordination::check_startup(&account_id)?;
    tokio::runtime::Runtime::new()?.block_on(async {
        let mut cfg=LiveNodeConfig{environment:Environment::Sandbox,trader_id:"SUSANTA-001".into(),instance_id:Some(run_id),cache:Some(persistence::cache_config()),save_state:true,load_state:false,shutdown_on_error:true,delay_post_stop:Duration::from_secs(2),..Default::default()};
        cfg.logging=LoggerConfig{stdout_level:log::LevelFilter::Warn,is_colored:false,..Default::default()};
        cfg.exec_engine.reconciliation=false;
        cfg.risk_engine.max_notional_per_order.insert(instrument.id.to_string(),"2000000".into());
        let builder=LiveNodeBuilder::from_config(cfg)?.with_cache_database_factory(Box::new(super::redis_cache::Factory(redis)))
            .add_data_client(Some("STBARS".into()),Box::new(live_data::Factory),Box::new(feed))?
            .add_data_client(Some("KITE".into()),Box::new(data::Factory),Box::new(data::Config{instrument:instrument.clone(),token:selection.instrument_token,seconds:seconds+10,synthetic_tick_ms:500,short_fixture:false,sandbox_user:None,credentials:Some(credentials)}))?;
        let mut node=builder.add_exec_client(Some("MCX".into()),Box::new(kite_adapter::execution::native_client::mock::MockFactory),Box::new(kite_adapter::execution::native_client::mock::MockConfig{namespace:run_id.to_string(),account_id:account_id.clone(),stop_signal:control.done.clone(),product:"MIS".into(),instrument_id:selection.instrument.clone(),symbol:selection.symbol.clone(),instrument_token:selection.instrument_token,market_price:Some(market_price.clone())}))?.build()?;
        let bar_type:BarType=format!("{}-3-MINUTE-LAST-EXTERNAL",instrument.id).parse()?;
        node.add_strategy(IlrcActor::new(bar_type,selection,data::now(),market_price).with_control(control.clone()))?;
        let handle=node.handle();
        let watcher_control=control.clone();
        let watcher=tokio::spawn(async move {
            tokio::select!{_ = tokio::signal::ctrl_c()=>{},_ = tokio::time::sleep(Duration::from_secs(seconds))=>{}}
            watcher_control.stopping.store(true,std::sync::atomic::Ordering::Release);
            for _ in 0..120 {
                if watcher_control.flat.load(std::sync::atomic::Ordering::Acquire){break;}
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            handle.stop();
        });
        println!("{}",serde_json::json!({"event":"ilrc_nautilus_mock_started","namespace":run_id.to_string(),"instrument":instrument.id.to_string(),"live_orders_enabled":false,"real_broker_orders_sent":false,"execution":"Kite native mock + Redis ownership"}));
        let result=node.run_with_mode(NodeRunMode::Hosted).await;
        watcher.abort();
        result?;
        println!("{}",serde_json::json!({"event":"ilrc_nautilus_mock_finished","namespace":run_id.to_string(),"real_broker_orders_sent":false}));
        Ok(())
    })
}

pub fn run_fixture(config: &str, fixture: &str, seconds: u64) -> Result<()> {
    ensure!(
        (5..=86360).contains(&seconds),
        "ILRC mock duration out of range"
    );
    let selection = Selection::load(config)?;
    let now = chrono::Utc::now().with_timezone(&chrono::FixedOffset::east_opt(19800).expect("IST"));
    let date = now.date_naive();
    ensure!(
        selection.session_calendar.session(date)?.is_some(),
        "ILRC selected date is not a trading session"
    );
    let master = kite_adapter::http::instruments::download()?;
    let report = kite_adapter::preflight::run_selected(
        &selection.symbol,
        selection.instrument_token,
        &master[..],
        date,
    )?;
    ensure!(
        report.instrument_id == selection.instrument
            && report.expiry == selection.expected_expiry.to_string(),
        "Live Kite instrument master does not match selected contract"
    );
    let instrument = kite_adapter::instruments::contract::build(&report, data::now().into())?;
    let raw: Vec<kite_adapter::http::historical::Candle> =
        serde_json::from_slice(&std::fs::read(fixture)?)?;
    let split = raw
        .iter()
        .position(|c| {
            c.time().is_ok_and(|t| {
                t.date_naive() == chrono::NaiveDate::from_ymd_opt(2026, 10, 7).expect("date")
                    && t.time() >= chrono::NaiveTime::from_hms_opt(14, 30, 0).expect("time")
            })
        })
        .ok_or_else(|| anyhow::anyhow!("Fixture missing October 7 replay boundary"))?;
    ensure!(
        split > 110 && raw.len() > split + 10,
        "Replay fixture too short"
    );
    let warmup = raw[..split].to_vec();
    let simulated = raw[split..].to_vec();
    let start_ns = live_bars::close_for(warmup.last().expect("warmup"), selection.interval)?;
    let run_id = UUID4::new();
    let market_price = Arc::new(AtomicI64::new(0));
    let control = Control::new(true).with_bar_ns(selection.interval.nanoseconds());
    let feed = live_data::Config {
        instrument: instrument.clone(),
        token: selection.instrument_token,
        date,
        calendar: selection.session_calendar.clone(),
        interval: selection.interval,
        volume_sensitive: false,
        synthetic_delay_ms: 10,
        warmup,
        simulated,
        control: control.clone(),
        emit_intrabar_ticks: false,
    };
    let redis = persistence::redis_config()?;
    let account_id = format!(
        "ILRC{}",
        run_id
            .to_string()
            .replace("-", "")
            .chars()
            .take(14)
            .collect::<String>()
    );
    kite_adapter::execution::native_client::coordination::check_startup(&account_id)?;
    tokio::runtime::Runtime::new()?.block_on(async {
        let mut cfg=LiveNodeConfig{environment:Environment::Sandbox,trader_id:"SUSANTA-001".into(),instance_id:Some(run_id),cache:Some(persistence::cache_config()),save_state:true,load_state:false,shutdown_on_error:true,delay_post_stop:Duration::from_secs(2),..Default::default()};
        cfg.logging=LoggerConfig{stdout_level:log::LevelFilter::Warn,is_colored:false,..Default::default()};
        cfg.exec_engine.reconciliation=false;
        cfg.risk_engine.max_notional_per_order.insert(instrument.id.to_string(),"2000000".into());
        let builder=LiveNodeBuilder::from_config(cfg)?.with_cache_database_factory(Box::new(super::redis_cache::Factory(redis)))
            .add_data_client(Some("STBARS".into()),Box::new(live_data::Factory),Box::new(feed))?;
        let mut node=builder.add_exec_client(Some("MCX".into()),Box::new(kite_adapter::execution::native_client::mock::MockFactory),Box::new(kite_adapter::execution::native_client::mock::MockConfig{namespace:run_id.to_string(),account_id:account_id.clone(),stop_signal:control.done.clone(),product:"MIS".into(),instrument_id:selection.instrument.clone(),symbol:selection.symbol.clone(),instrument_token:selection.instrument_token,market_price:Some(market_price.clone())}))?.build()?;
        let bar_type:BarType=format!("{}-3-MINUTE-LAST-EXTERNAL",instrument.id).parse()?;
        node.add_strategy(IlrcActor::new(bar_type,selection,start_ns,market_price).with_control(control.clone()))?;
        let handle=node.handle();
        let watcher_control=control.clone();
        let watcher=tokio::spawn(async move {
            tokio::select!{_ = tokio::signal::ctrl_c()=>{},_ = tokio::time::sleep(Duration::from_secs(seconds))=>{}}
            watcher_control.stopping.store(true,std::sync::atomic::Ordering::Release);
            for _ in 0..120 {
                if watcher_control.flat.load(std::sync::atomic::Ordering::Acquire){break;}
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            handle.stop();
        });
        println!("{}",serde_json::json!({"event":"ilrc_nautilus_fixture_started","namespace":run_id.to_string(),"instrument":instrument.id.to_string(),"live_orders_enabled":false,"real_broker_orders_sent":false,"execution":"Kite native mock + Redis ownership"}));
        let result=node.run_with_mode(NodeRunMode::Hosted).await;
        watcher.abort();
        result?;
        println!("{}",serde_json::json!({"event":"ilrc_nautilus_fixture_finished","namespace":run_id.to_string(),"real_broker_orders_sent":false}));
        Ok(())
    })
}

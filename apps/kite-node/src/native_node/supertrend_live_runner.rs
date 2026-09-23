//! Selected strategy: paper, native mock, or explicitly gated production execution.
use super::{
    data, persistence,
    supertrend_actor::{BarStrategy, State},
    supertrend_live_bars as bars,
    supertrend_live_control::Control,
    supertrend_live_data as feed,
    supertrend_live_lease::Lease,
};
use anyhow::{Result, ensure};
use kite_adapter::http::historical::{Candle, Interval};
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
fn synthetic(interval: Interval) -> Result<(Vec<Candle>, Vec<Candle>)> {
    let step = interval.nanoseconds();
    let first = data::now() / step * step - 160 * step;
    let mut candles = Vec::new();
    for i in 0..160u64 {
        let x = i % 40;
        let p = 6000.
            + if x < 20 {
                x as f64 * 10.
            } else {
                (40 - x) as f64 * 10.
            };
        let timestamp = chrono::DateTime::from_timestamp_nanos((first + i * step) as i64)
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
fn pivot_synthetic(selection: &super::production::Selection) -> Result<(Vec<Candle>, Vec<Candle>)> {
    let date = selection.session_calendar.range(
        selection.session_calendar.valid_from,
        selection.session_calendar.valid_through,
    )?[0];
    let (first, end) = selection.session_calendar.bounds(date)?;
    let step = selection.bar_ns();
    let count = ((end - first) / step) as usize;
    ensure!(
        count >= 140,
        "Pivot simulation fixture needs a regular full session"
    );
    let mut candles = Vec::new();
    for i in 0..count {
        let x = i % 40;
        let p = 6000.
            + if x < 20 {
                x as f64 * 10.
            } else {
                (40 - x) as f64 * 10.
            };
        let timestamp = chrono::DateTime::from_timestamp_nanos((first + i as u64 * step) as i64)
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
    let live = candles.split_off(count - 40);
    Ok((candles, live))
}
pub fn run(config: &str, seconds: u64, sim: bool) -> Result<()> {
    run_with_execution(config, seconds, sim, false)
}
pub fn run_with_execution(config: &str, seconds: u64, sim: bool, kite_mock: bool) -> Result<()> {
    run_backend(config, seconds, sim, kite_mock, None, false)
}
pub fn run_recovery_fixture(config: &str) -> Result<()> {
    run_backend(config, 30, true, false, None, true)
}
pub fn run_broker(config: &str, settings: &str) -> Result<()> {
    let selection = super::production::Selection::load(config)?;
    let settings = selection.broker_settings(settings)?;
    run_backend(
        config,
        selection.production_duration(data::now())?,
        false,
        false,
        Some(settings),
        false,
    )
}
fn run_backend(
    config: &str,
    seconds: u64,
    sim: bool,
    kite_mock: bool,
    production: Option<kite_adapter::execution::native_client::production::Settings>,
    recovery_fixture: bool,
) -> Result<()> {
    let alerts = super::slack_alerts::Alerts::from_env(sim)?;
    let mode = if production.is_some() {
        "PRODUCTION"
    } else {
        "PAPER/MOCK"
    };
    let selected = super::production::Selection::load(config)?;
    let interval_minutes = selected.interval_minutes();
    let symbol = selected.symbol;
    let strategy = selected.strategy;
    alerts.emit(format!(
        "{symbol} {interval_minutes}m {strategy} [{mode}]: STARTING; initialization in progress"
    ));
    let result = run_backend_inner(
        config,
        seconds,
        sim,
        kite_mock,
        production,
        recovery_fixture,
        &alerts,
    );
    alerts.emit(format!("{symbol} {interval_minutes}m {strategy} [{mode}]: {}", if result.is_ok() {
        "CLEAN STOP: flat, no pending orders; inspect saved run report"
    } else {
        "FAILED / REVIEW REQUIRED: inspect terminal logs and Redis before restart; do not assume flat"
    }));
    result
}
fn run_backend_inner(
    config: &str,
    seconds: u64,
    sim: bool,
    kite_mock: bool,
    production: Option<kite_adapter::execution::native_client::production::Settings>,
    recovery_fixture: bool,
    alerts: &super::slack_alerts::Alerts,
) -> Result<()> {
    let real = production.is_some();
    if let Some(s) = &production {
        s.validate()?;
    }
    super::supertrend_terminal::step("Checking strategy selection and duration");
    let selection = super::production::Selection::load(config)?;
    ensure!(
        (5..=86360).contains(&seconds),
        "Run duration must be 5..86360 seconds"
    );
    let date = chrono::Utc::now()
        .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
        .date_naive();
    super::supertrend_terminal::step("Resolving instrument and data credentials");
    let (instrument, token, credentials) = if sim {
        (
            selection.synthetic_instrument()?,
            selection.instrument_token,
            None,
        )
    } else {
        let master = kite_adapter::http::instruments::download()?;
        let report = selection.resolve(&master, date)?;
        (
            kite_adapter::instruments::contract::build(&report, data::now().into())?,
            report.instrument_token,
            Some(Arc::new(kite_adapter::credentials::redis::load_from_env()?)),
        )
    };
    super::supertrend_terminal::step(&format!(
        "Loading completed {}-minute candles for warmup",
        selection.interval_minutes()
    ));
    let (warmup, simulated) = if sim {
        if selection.pivot_point.is_some() || selection.trend_ribbon.is_some() {
            pivot_synthetic(&selection)?
        } else {
            synthetic(selection.interval)?
        }
    } else {
        let raw = tokio::runtime::Runtime::new()?.block_on(
            kite_adapter::http::historical::fetch_window_for(token, date, 7, selection.interval),
        )?;
        (
            bars::completed_for(raw, data::now(), selection.interval)?,
            Vec::new(),
        )
    };
    ensure!(
        warmup.len()
            >= selection.pivot_point.as_ref().map_or_else(
                || selection.trend_ribbon.as_ref().map_or(100, |r| 100
                    .max(r.alma_length.max(r.deviation_length).max(r.atr_length) + r.slope_length)),
                |p| 100.max(p.atr_period)
            ),
        "Insufficient indicator warmup"
    );
    if !sim {
        bars::validate_warmup_for(
            &warmup,
            date,
            data::now(),
            &selection.session_calendar,
            selection.interval,
        )?;
    }
    let (start, end) = if sim {
        (
            bars::close_for(warmup.last().unwrap(), selection.interval)?,
            u64::MAX,
        )
    } else {
        let (session_start, end) = selection.execution_bounds(date, real)?;
        ensure!(
            data::now() >= session_start && data::now() + 15_000_000_000 < end,
            "Start the strategy during the configured execution session"
        );
        (data::now(), end)
    };
    let seconds = if sim {
        seconds
    } else {
        let remaining = end.saturating_sub(data::now());
        seconds.min(
            if selection.pivot_point.is_some() || selection.trend_ribbon.is_some() {
                remaining.div_ceil(1_000_000_000)
            } else {
                (remaining / 1_000_000_000)
                    .saturating_sub(super::supertrend_session::EXIT_BUFFER_SECONDS)
            },
        )
    };
    ensure!(seconds >= 5, "Too close to session end to start");
    super::supertrend_terminal::step("Checking Redis persistence and strategy ownership");
    let redis = persistence::redis_config()?;
    if kite_mock || real {
        let account = production
            .as_ref()
            .map(|s| s.expected_user_id.as_str())
            .unwrap_or("MOCK");
        kite_adapter::execution::native_client::coordination::check_startup(account)?;
    }
    let id = UUID4::new();
    let mut control = Control::new(sim).with_bar_ns(selection.bar_ns());
    control.real = real;
    control.recovery_fixture = recovery_fixture;
    let state = Rc::new(RefCell::new(State::default()));
    let mut lease = Lease::acquire(&id.to_string(), sim, real, &selection.symbol)?;
    let feed_config = feed::Config {
        instrument: instrument.clone(),
        token,
        date,
        calendar: selection.session_calendar.clone(),
        interval: selection.interval,
        volume_sensitive: selection.trend_ribbon.is_none(),
        synthetic_delay_ms: if kite_mock { 180 } else { 60 },
        warmup: warmup.clone(),
        simulated,
        control: control.clone(),
    };
    let outcome=tokio::runtime::Runtime::new()?.block_on(async {
        let mut cfg=LiveNodeConfig{environment:if real {Environment::Live}else{Environment::Sandbox},trader_id:"SUSANTA-001".into(),instance_id:Some(id),
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
        let mut node=if let Some(settings)=production.clone() {
            use kite_adapter::execution::native_client::production::{Factory,LiveConfig};
            builder.add_exec_client(Some("MCX".into()),Box::new(Factory),Box::new(LiveConfig{settings,instrument_id:selection.instrument.clone(),symbol:selection.symbol.clone(),namespace:id.to_string(),stop_signal:control.done.clone()}))?.build()?
        }else if kite_mock {
            use kite_adapter::execution::native_client::mock::{MockFactory,MockConfig};
            builder.add_exec_client(Some("MCX".into()),Box::new(MockFactory),Box::new(MockConfig{namespace:id.to_string(),stop_signal:control.done.clone(),product:"MIS".into(),instrument_id:selection.instrument.clone(),symbol:selection.symbol.clone(),instrument_token:token}))?.build()?
        }else{builder.add_simulated_exec_client(Some("MCX".into()),Box::new(SandboxExecutionClientFactory::new()),Box::new(simulation))?.build()?};
        let bt: BarType = selection.bar_type(&instrument.id)?;
        let mut strategy = BarStrategy::new(bt,start,end,state.clone()).with_confirmation(selection.pivot_point.is_none() && selection.trend_ribbon.is_none()).with_live(control.clone());
        if let Some(pivot) = &selection.pivot_point { strategy = strategy.with_pivot(pivot.clone(),selection.session_calendar.clone())?; }
        if let Some(ribbon) = &selection.trend_ribbon { strategy = strategy.with_ribbon_interval(ribbon.clone(), selection.session_calendar.clone(), selection.bar_ns())?; }
        node.add_strategy(strategy)?;
        let owner_monitor=lease.monitor(control.clone());
        let handle=node.handle();let ctl=control.clone();
        let watcher=tokio::spawn(async move {
            tokio::select!{_=super::lifecycle::wait(ctl.done.clone(),seconds)=>{},_=tokio::time::sleep(Duration::from_secs(seconds))=>{},_=async {
                loop {tokio::time::sleep(Duration::from_millis(100)).await;let deadline=ctl.order_deadline.load(Ordering::Acquire);if deadline>0 && data::now()>=deadline {ctl.fail("Order fill deadline exceeded; cancellation and review required");break;}}
            }=>{}}
            ctl.stop();
            tokio::time::sleep(Duration::from_millis(250)).await;
            for _ in 0..50 {
                if ctl.flat.load(Ordering::Acquire){break;}
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            handle.stop();
        });
        println!("{}",serde_json::json!({"event":"supertrend_live_started","namespace":id.to_string(),"instrument":instrument.id.to_string(),"instrument_token":token,"runtime":"LiveNode","strategy":selection.strategy,"interval":selection.interval_name(),"simulated_feed":sim,"execution":if real {"Kite production"}else if kite_mock{"Kite native mock"}else{"Nautilus Sandbox"},"warmup_bars":warmup.len(),"square_off_ns":if sim{None}else{Some(end)},"live_orders_enabled":real}));
        alerts.emit(format!(
            "{} {}m: run {id} initialized; real_orders={real}",
            selection.symbol,
            selection.interval_minutes()
        ));
        let mut was_paused=false;
        let display = super::supertrend_terminal::Display::new(
            seconds,
            warmup.len(),
            sim,
            &id.to_string(),
            real,
            kite_mock,
            &selection,
        )
        .with_strategy_start_ns(start);
        let result={
            let run=node.run_with_mode(nautilus_live::node::NodeRunMode::Hosted);
            tokio::pin!(run);
            let mut refresh=tokio::time::interval(Duration::from_secs(1));
            refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {tokio::select! {
                result=&mut run => break result,
                _=refresh.tick()=> {
                    display.render(&state.borrow(),&control);
                    let paused=control.paused.load(Ordering::Acquire);
                    if paused != was_paused {
                        alerts.emit(format!("CRUDEOIL run {id}: {}", if paused {"PAUSED: data recovery required"} else {"RESUMED: validated history rebuilt"}));
                        was_paused=paused;
                    }
                },
            }}
        };
        display.render(&state.borrow(),&control);
        watcher.abort();owner_monitor.abort();
        // LiveNode disposal disconnects execution clients and performs the final
        // broker reconciliation. Snapshot native state only after that completes;
        // otherwise a last shutdown fill is omitted from the report.
        node.dispose();
        result?;
        let position=state.borrow().cache.as_ref().map(|cache|cache.borrow().positions_open(None,None,None,None,None).iter().map(|p|p.signed_qty).sum::<f64>()).unwrap_or(0.);
        let pending=state.borrow().cache.as_ref().map(|cache|cache.borrow().orders_open(None,None,None,None,None).len()+cache.borrow().orders_inflight(None,None,None,None,None).len()).unwrap_or(0);
        Ok::<_,anyhow::Error>((position,pending))
    });
    if kite_mock || real {
        let scope = production
            .as_ref()
            .map(|s| s.expected_user_id.as_str())
            .unwrap_or("MOCK");
        match kite_adapter::execution::native_client::coordination::status(scope) {
            Ok(health) if health["state"] == "Clean" => {}
            _ => control.fail("Native account reconciliation requires review"),
        }
    }
    let fault = control.fault.lock().expect("fault lock").clone();
    // Failed initialization/reconciliation gives no trustworthy position or order count.
    // Serialize unknown values as null instead of inventing an open order.
    let (position, pending) = outcome
        .as_ref()
        .map(|&(position, pending)| (Some(position), Some(pending)))
        .unwrap_or((None, None));
    let clean = outcome.is_ok()
        && position == Some(0.)
        && pending == Some(0)
        && fault.is_none()
        && state.borrow().errors.is_empty()
        && state.borrow().stopped;
    lease.finish(clean, position.unwrap_or(f64::NAN))?;
    let folder = std::path::PathBuf::from(format!("data/supertrend-live/{id}"));
    std::fs::create_dir_all(&folder)?;
    let s = state.borrow();
    super::backtest_report::json(&folder, "indicators.json", &s.indicators)?;
    super::backtest_report::json(&folder, "signals.json", &s.signals)?;
    super::backtest_report::json(&folder, "fills.json", &s.fills)?;
    super::backtest_report::json(&folder, "recoveries.json", &s.rebuilds)?;
    let bar_feed = control.bar_feed.lock().expect("bar feed stats").clone();
    super::backtest_report::json(&folder, "bar-feed.json", &bar_feed)?;
    let output = serde_json::json!({"event":"supertrend_live_complete","namespace":id.to_string(),"instrument":instrument.id.to_string(),"instrument_token":token,"status":if clean{"Clean"}else{"ReviewRequired"},
        "runtime":"LiveNode","strategy":selection.strategy,"interval":selection.interval_name(),"contracts":1,"atr_stop_enabled":false,
        "simulated_feed":sim,"execution":if real {"Kite production"}else if kite_mock{"Kite native mock"}else{"Nautilus Sandbox"},"quotes":s.live_quotes,"rejected_quotes":s.rejected_quotes,
        "bars":s.indicators.len(),"warmup_bars":warmup.len(),"signals":s.signals.len(),"fills":s.fills.len(),"open_contracts":position,
        "open_orders":pending,"errors":s.errors,"feed_fault":fault,"run_error":outcome.as_ref().err().map(ToString::to_string),
        "bar_feed":bar_feed,"square_off_ns":if sim{None}else{Some(end)},"indicator_rebuilds":control.recoveries.load(Ordering::Acquire),"automatic_resume_enabled":false,"report_directory":folder,"live_orders_enabled":real,"broker_orders_sent":if real {serde_json::Value::Null}else{serde_json::json!(false)}});
    super::backtest_report::json(&folder, "summary.json", &output)?;
    println!("{output}");
    super::supertrend_terminal::finish(clean, &folder, real);
    outcome?;
    ensure!(clean, "Strategy run requires recovery review");
    Ok(())
}

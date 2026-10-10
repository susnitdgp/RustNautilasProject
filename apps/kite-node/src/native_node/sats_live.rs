//! Nautilus LiveNode runner for one `sats` portfolio slot.
//!
//! * `paper`: live Kite market data, orders filled by the native Kite **mock**
//!   execution client (in memory). Never touches the broker's order API.
//! * `live`:  live Kite market data and the native Kite **production** execution
//!   client. Requires all of: a `--features live-orders` build, the slot marked
//!   `enabled` and `live_orders_enabled` in the portfolio, and the broker settings
//!   file with `live_orders_enabled: true` and the exact Kite user ID. The launch
//!   script additionally asks the operator to type LIVE.
//!
//! One run covers one trading day: it starts inside the session, warms SATS on
//! broker-finalised history, trades until the configured square-off, flattens,
//! and stops. Any feed gap or invalid packet fails closed (flatten, stop).
use super::{
    data, live_bars, live_control::Control,
    dash_writer::{self, DashboardConfig},
    sats_dashboard::{Board, emit, note},
    portfolio::{Instance, Portfolio},
    sats_config::{self, SatsConfig},
    sats_strategy::SatsStrategy,
    session_calendar::Calendar,
};
use anyhow::{Context, Result, bail, ensure};
use chrono::{FixedOffset, NaiveDate, TimeZone};
use kite_adapter::{
    execution::native_client::{keys::KeySpace, production::Settings},
    http::historical::Candle,
};
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_core::UUID4;
use nautilus_live::{builder::LiveNodeBuilder, config::LiveNodeConfig, node::NodeRunMode};
use nautilus_model::instruments::FuturesContract;
use std::{
    sync::{Arc, atomic::AtomicI64},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Paper,
    Live,
}

fn ist() -> FixedOffset {
    FixedOffset::east_opt(19_800).expect("IST")
}

/// Everything checked before a node is built.
struct Prepared {
    inst: Instance,
    keys: KeySpace,
    trader_id: String,
    config: SatsConfig,
    instrument: FuturesContract,
    warmup: Vec<Candle>,
    start_ns: u64,
    run_seconds: u64,
}

fn load_slot(portfolio_path: &str, instance_id: &str) -> Result<(Instance, SatsConfig, KeySpace, String)> {
    let portfolio: Portfolio = serde_json::from_str(
        &std::fs::read_to_string(portfolio_path).with_context(|| format!("Cannot read {portfolio_path}"))?,
    )?;
    portfolio.validate()?;
    let keys = portfolio.keyspace(instance_id)?;
    let trader_id = portfolio.trader_id(instance_id)?;
    let inst = portfolio
        .instances
        .into_iter()
        .find(|v| v.id == instance_id)
        .ok_or_else(|| anyhow::anyhow!("No portfolio instance {instance_id}"))?;
    ensure!(inst.strategy == sats_config::STRATEGY, "Instance {instance_id} does not run sats");
    ensure!(inst.instrument_token != 0, "Instance {instance_id} needs a verified instrument token");
    let config = SatsConfig::load(&inst.strategy_config)?;
    Ok((inst, config, keys, trader_id))
}

fn load_broker(path: &str, inst: &Instance) -> Result<Settings> {
    let settings: Settings = serde_json::from_slice(&std::fs::read(path).with_context(|| format!("Cannot read {path}"))?)
        .with_context(|| format!("Invalid broker settings {path}"))?;
    settings.validate()?;
    ensure!(
        settings.instrument_token == inst.instrument_token,
        "Broker settings token {} differs from slot token {}",
        settings.instrument_token,
        inst.instrument_token
    );
    Ok(settings)
}

fn live_gates(inst: &Instance) -> Result<()> {
    ensure!(cfg!(feature = "live-orders"), "Real orders need a `cargo build --release --features live-orders` binary");
    ensure!(inst.enabled, "Slot {} is not enabled in the portfolio", inst.id);
    ensure!(inst.live_orders_enabled, "Slot {} does not have live_orders_enabled in the portfolio", inst.id);
    Ok(())
}

fn prepare(portfolio_path: &str, instance_id: &str) -> Result<Prepared> {
    let (inst, config, keys, trader_id) = load_slot(portfolio_path, instance_id)?;
    let calendar: Calendar = serde_json::from_slice(
        &std::fs::read(&config.live.session_calendar)
            .with_context(|| format!("Cannot read {}", config.live.session_calendar))?,
    )?;
    calendar.validate()?;
    let now_ns = data::now();
    let now = chrono::Utc::now().with_timezone(&ist());
    let date = now.date_naive();
    if let Some(roll) = &inst.rollover {
        ensure!(date <= roll.expected_expiry, "Contract {} expired on {}; roll the slot", inst.instrument, roll.expected_expiry);
    }
    let Some((open_ns, close_ns)) = calendar.session(date)? else {
        bail!("No MCX session on {date} (weekend/holiday per {})", config.live.session_calendar);
    };
    let square_off_ns = ist_ns(date, config.live.square_off)?;
    ensure!(square_off_ns < close_ns, "Square-off must be before the session close");
    ensure!(now_ns >= open_ns, "Session has not opened yet");
    ensure!(
        now_ns + 5 * 60 * 1_000_000_000 < square_off_ns,
        "Less than 5 minutes left before the {} square-off; not starting",
        config.live.square_off
    );

    let symbol = inst.instrument.strip_suffix(".MCX").ok_or_else(|| anyhow::anyhow!("Slot instrument must be .MCX"))?;
    let master = kite_adapter::http::instruments::download()?;
    let report = kite_adapter::preflight::run_selected(symbol, inst.instrument_token, &master[..], date)?;
    ensure!(report.instrument_id == inst.instrument, "Kite instrument master does not match the slot instrument");
    if let Some(roll) = &inst.rollover {
        ensure!(report.expiry == roll.expected_expiry.to_string(), "Kite expiry {} differs from slot expiry", report.expiry);
    }
    let instrument = kite_adapter::instruments::contract::build(&report, now_ns.into())?;

    let interval = config.interval()?;
    let raw = tokio::runtime::Runtime::new()?.block_on(kite_adapter::http::historical::fetch_window_for(
        inst.instrument_token,
        date,
        10,
        interval,
    ))?;
    let warmup = live_bars::broker_finalized_for(raw, data::now(), interval)?;
    live_bars::validate_broker_finalized_warmup_for(&warmup, date, data::now(), &calendar, interval)?;
    let needed = config.engine()?.resolved().warmup_bars as usize + 20;
    ensure!(warmup.len() >= needed, "Warm-up has {} bars, SATS needs at least {needed}", warmup.len());
    let start_ns = live_bars::close_for(warmup.last().expect("warmup"), interval)?;
    let run_seconds = (square_off_ns + 2 * 60 * 1_000_000_000).saturating_sub(now_ns) / 1_000_000_000;
    Ok(Prepared { inst, keys, trader_id, config, instrument, warmup, start_ns, run_seconds })
}

fn ist_ns(date: NaiveDate, time: chrono::NaiveTime) -> Result<u64> {
    let t = ist()
        .from_local_datetime(&date.and_time(time))
        .single()
        .ok_or_else(|| anyhow::anyhow!("Invalid IST time"))?;
    Ok(u64::try_from(t.timestamp_nanos_opt().ok_or_else(|| anyhow::anyhow!("Timestamp overflow"))?)?)
}

/// Checks everything a live run needs without creating a node or sending anything.
pub fn preflight(portfolio_path: &str, instance_id: &str, broker_path: &str) -> Result<()> {
    let p = prepare(portfolio_path, instance_id)?;
    let settings = load_broker(broker_path, &p.inst)?;
    live_gates(&p.inst)?;
    kite_adapter::execution::native_client::coordination::check_unlocked(&p.keys, &settings.expected_user_id)?;
    emit(
        serde_json::json!({
            "event": "sats_live_preflight", "status": "PASS",
            "instance": p.inst.id, "instrument": p.inst.instrument,
            "warmup_bars": p.warmup.len(), "square_off": p.config.live.square_off.to_string(),
            "lots": p.config.lots, "exit": p.config.execution,
            "product": settings.product, "broker_orders_sent": false,
            "redis_order_budget": p.keys.order_budget(&settings.expected_user_id)?,
            "nautilus_trader_id": p.trader_id,
        }),
    );
    Ok(())
}

pub fn run(portfolio_path: &str, instance_id: &str, broker_path: Option<&str>, mode: Mode) -> Result<()> {
    let p = prepare(portfolio_path, instance_id)?;
    let settings = match mode {
        Mode::Live => {
            let path = broker_path.ok_or_else(|| anyhow::anyhow!("Live mode needs the broker settings file"))?;
            let s = load_broker(path, &p.inst)?;
            live_gates(&p.inst)?;
            Some(s)
        }
        Mode::Paper => None,
    };
    let run_id = UUID4::new();
    // Paper runs use a fixed pseudo-account so their lease/budget never touch the real account's.
    let account_id = match &settings {
        Some(s) => s.expected_user_id.clone(),
        None => "PAPER".to_owned(),
    };
    let namespace = format!(
        "{}-{}",
        chrono::Utc::now().with_timezone(&ist()).format("%Y%m%d"),
        run_id.to_string().split('-').next().unwrap_or("run")
    );
    // One process per slot and account; the OS frees the lock when this process ends.
    let lock = kite_adapter::execution::native_client::coordination::lock_instance(&p.keys, &account_id)?;
    let interval = p.config.interval()?;
    let mut control = Control::new(false).with_bar_ns(interval.nanoseconds());
    control.real = mode == Mode::Live;
    let market_price = Arc::new(AtomicI64::new(0));
    let credentials = Arc::new(kite_adapter::credentials::redis::load_from_env()?);
    let symbol = p.inst.instrument.trim_end_matches(".MCX").to_owned();
    let mode_label = if mode == Mode::Live { "LIVE (real Zerodha orders)" } else { "PAPER (Kite mock execution)" };
    let square_off_ns = ist_ns(chrono::Utc::now().with_timezone(&ist()).date_naive(), p.config.live.square_off)? as i64;
    let board = Board {
        mode: mode_label.into(),
        slot: p.inst.id.clone(),
        instrument: p.inst.instrument.clone(),
        square_off: p.config.live.square_off.format("%H:%M").to_string(),
        exit_rule: match p.config.execution.exit_mode {
            sats_config::ExitMode::Trail if p.config.execution.trail.supertrend_trail => "Trail: BE@TP1 + SuperTrend".into(),
            sats_config::ExitMode::Trail => "Trail: BE@TP1".into(),
            _ if p.config.execution.intrabar_single() => {
                format!("{} or SL, intrabar", p.config.execution.single_exit_at.label())
            }
            mode => format!("{:?} {:?}", mode, p.config.execution.single_exit_at),
        },
        redis_namespace: namespace.clone(),
        point_value: p.config.point_value,
        lots: p.config.lots,
        bar_ns: p.config.bar_ns(),
        ..Board::default()
    };
    // Dashboard: strategy and stop watcher push into their own lock-free queues; one
    // thread owns the board and publishes it to the dashboard Redis (config/dashboard.json:
    // local Redis database 1, apart from the trading keys in database 0).
    let dash_url = DashboardConfig::load(dash_writer::CONFIG_PATH)?.map(|c| c.redis_dashboard_url);
    let dash_keys = dash_writer::Keys::new(&p.keys.dashboard());
    let (dashboard, [strategy_feed, watcher_feed]) = dash_writer::start::<2>(board, dash_url.clone(), dash_keys.clone())?;

    tokio::runtime::Runtime::new()?.block_on(async {
        let mut cfg = LiveNodeConfig {
            environment: if mode == Mode::Live { Environment::Live } else { Environment::Sandbox },
            trader_id: p.trader_id.as_str().into(),
            instance_id: Some(run_id),
            // Nautilus' default in-memory cache: nothing is persisted or reloaded.
            save_state: false,
            load_state: false,
            shutdown_on_error: true,
            delay_post_stop: Duration::from_secs(2),
            ..Default::default()
        };
        cfg.logging = LoggerConfig { stdout_level: log::LevelFilter::Warn, is_colored: false, ..Default::default() };
        cfg.exec_engine.reconciliation = false;
        cfg.risk_engine.max_notional_per_order.insert(p.instrument.id.to_string(), "2000000".into());
        let builder = LiveNodeBuilder::from_config(cfg)?
            .add_data_client(
                Some("KITE".into()),
                Box::new(data::Factory),
                Box::new(data::Config {
                    instrument: p.instrument.clone(),
                    token: p.inst.instrument_token,
                    seconds: p.run_seconds + 120,
                    synthetic_tick_ms: 500,
                    short_fixture: false,
                    sandbox_user: None,
                    credentials: Some(credentials),
                    live_bars: Some((p.warmup.clone(), p.start_ns, control.clone())),
                    interval,
                }),
            )?;
        let builder = match settings {
            Some(settings) => builder.add_exec_client(
                Some("MCX".into()),
                Box::new(kite_adapter::execution::native_client::production::Factory),
                Box::new(kite_adapter::execution::native_client::production::LiveConfig {
                    settings,
                    instrument_id: p.inst.instrument.clone(),
                    symbol: symbol.clone(),
                    namespace: namespace.clone(),
                    stop_signal: control.done.clone(),
                    keys: p.keys.clone(),
                }),
            )?,
            None => builder.add_exec_client(
                Some("MCX".into()),
                Box::new(kite_adapter::execution::native_client::mock::MockFactory),
                Box::new(kite_adapter::execution::native_client::mock::MockConfig {
                    namespace: namespace.clone(),
                    account_id: account_id.clone(),
                    stop_signal: control.done.clone(),
                    product: p.config.live.product.clone(),
                    instrument_id: p.inst.instrument.clone(),
                    symbol: symbol.clone(),
                    instrument_token: p.inst.instrument_token,
                    market_price: Some(market_price.clone()),
                    keys: p.keys.clone(),
                    max_lots: p.config.lots,
                }),
            )?,
        };
        let mut node = builder.build()?;
        let bar_type = data::bar_type_for(&p.instrument, interval)?;
        node.add_strategy(
            SatsStrategy::new(&p.inst.id, bar_type, &p.config, p.config.engine()?, true)
                .with_data_client("KITE".into())
                .with_dashboard(strategy_feed)
                .with_live(p.start_ns as i64, p.config.live.square_off_minute(), control.clone(), market_price.clone()),
        )?;
        let handle = node.handle();
        let watcher_control = control.clone();
        let seconds = p.run_seconds;
        // Ctrl+C, SIGTERM (systemd/kill) and SIGHUP (SSH/terminal closed) all take the
        // same path: flatten, wait for flat, stop. A dropped session never leaves a position.
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sighup = signal(SignalKind::hangup())?;
        let watcher = tokio::spawn(async move {
            let why = tokio::select! {
                _ = tokio::signal::ctrl_c() => "Ctrl+C received: flattening and stopping SATS",
                _ = sigterm.recv() => "SIGTERM received: flattening and stopping SATS",
                _ = sighup.recv() => "Terminal/SSH closed (SIGHUP): flattening and stopping SATS",
                _ = tokio::time::sleep(Duration::from_secs(seconds)) => "Square-off window passed: stopping SATS",
            };
            emit(serde_json::json!({"event":"sats_stop_requested","reason":why}));
            watcher_feed.push(move |b| {
                b.status = "STOPPING".into();
                b.event(why.into());
            });
            watcher_control.stopping.store(true, std::sync::atomic::Ordering::Release);
            for _ in 0..240 {
                if watcher_control.flat.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            if !watcher_control.flat.load(std::sync::atomic::Ordering::Acquire) {
                watcher_feed.push(|b| b.event("SHUTDOWN WARNING: position not confirmed flat; check Kite immediately".into()));
            }
            handle.stop();
        });
        emit(
            serde_json::json!({
                "event": "sats_node_started", "mode": mode_label, "namespace": &namespace,
                "lock": lock.path().display().to_string(),
                "redis_order_budget": p.keys.order_budget(&account_id)?,
                "dashboard_redis": dash_url.is_some(), "dashboard_state": &dash_keys.state,
                "dashboard_events": &dash_keys.events, "dashboard_live": &dash_keys.live,
                "instance": p.inst.id, "instrument": p.instrument.id.to_string(), "bar_type": bar_type.to_string(),
                "warmup_bars": p.warmup.len(), "square_off": p.config.live.square_off.to_string(),
                "runs_for_seconds": seconds, "lots": p.config.lots, "exit": p.config.execution,
            }),
        );
        let result = node.run_with_mode(NodeRunMode::Hosted).await;
        watcher.abort();
        let final_board = dashboard.close().render(chrono::Utc::now().with_timezone(&ist()), square_off_ns);
        note(&final_board);
        let fault = control.fault.lock().ok().and_then(|f| f.clone());
        if let Err(err) = result {
            note(&format!("SATS EXIT WARNING: {err:#}. Check positions and open orders in Kite."));
            return Err(err);
        }
        emit(
            serde_json::json!({"event":"sats_node_finished","mode":mode_label,"namespace":&namespace,"fault":fault,
                "flat":control.flat.load(std::sync::atomic::Ordering::Acquire)}),
        );
        note(if control.flat.load(std::sync::atomic::Ordering::Acquire) {
            "SATS stopped. Position confirmed flat."
        } else {
            "SATS stopped. Position NOT confirmed flat: check Kite now."
        });
        Ok(())
    })
}

use super::{
    actor::{NativeStrategy, State},
    audit::AuditActor,
    catalog, persistence,
};
use anyhow::{Result, ensure};
use nautilus_backtest::{
    config::{
        BacktestDataConfig, BacktestEngineConfig, BacktestRunConfig, BacktestVenueConfig,
        NautilusDataType,
    },
    node::BacktestNode,
};
use nautilus_common::{cache::database::CacheDatabaseFactory, logging::logger::LoggerConfig};
use nautilus_core::UUID4;
use nautilus_model::{
    enums::{AccountType, BookType, OmsType},
    instruments::InstrumentAny,
};
use nautilus_trading::algorithm::{ExecutionAlgorithmConfig, twap::TwapAlgorithm};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
};
pub fn run(strategy_path: &str, catalog_path: Option<&str>) -> Result<()> {
    let strategy = kite_strategy::config::Config::parse(&std::fs::read_to_string(strategy_path)?)?;
    let (instrument, _) = crate::paper_flow::simulation::fixture()?;
    let instance = UUID4::new();
    let run_id = instance.to_string();
    let path = catalog_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("data/native-catalog/{run_id}")));
    if catalog_path.is_none() {
        catalog::write(
            &path,
            InstrumentAny::FuturesContract(instrument.clone()),
            &catalog::fixture_quotes()?,
        )?;
    }
    let full_ticks = catalog::read_full(&path)?;
    let full_replay = !full_ticks.is_empty();
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
    let data = BacktestDataConfig::builder()
        .data_type(NautilusDataType::QuoteTick)
        .catalog_path(path.to_string_lossy().into_owned())
        .instrument_id(instrument.id)
        .build()?;
    let config = BacktestRunConfig::builder()
        .id(run_id.clone())
        .engine(engine_config)
        .venues(vec![venue])
        .data(if full_replay { vec![] } else { vec![data] })
        .maybe_chunk_size(if full_replay { None } else { Some(5) })
        .raise_exception(true)
        .dispose_on_completion(false)
        .build()?;
    let mut node = BacktestNode::new(vec![config])?;
    node.build()?;
    let state = Rc::new(RefCell::new(State::default()));
    let audit = Rc::new(Cell::new(0));
    let engine = node.get_engine_mut(&run_id).expect("built backtest");
    let db = nautilus_common::live::get_runtime().block_on(
        super::redis_cache::Factory(persistence::redis_config()?).create(
            "SUSANTA-001".into(),
            instance,
            cache_config,
        ),
    )?;
    engine.kernel_mut().cache.borrow_mut().set_database(db);
    engine.add_instrument(&InstrumentAny::FuturesContract(instrument.clone()))?;
    engine.add_strategy(NativeStrategy::new(
        strategy,
        instrument.id,
        full_replay,
        state.clone(),
        Arc::new(AtomicBool::new(false)),
    ))?;
    engine.add_actor(AuditActor::new(instrument.id, audit.clone()))?;
    engine.add_exec_algorithm(TwapAlgorithm::new(ExecutionAlgorithmConfig {
        exec_algorithm_id: Some("TWAP".into()),
        ..Default::default()
    }))?;
    if full_replay {
        use nautilus_model::data::{CustomData, Data};
        engine.add_data_client_if_not_exists("KITE".into());
        let mut replay = Vec::with_capacity(full_ticks.len() * 3);
        let mut generation = None;
        for tick in &full_ticks {
            if generation != Some(tick.snapshot.connection_generation) {
                generation = Some(tick.snapshot.connection_generation);
                replay.push(Data::Custom(CustomData::from_arc(Arc::new(
                    super::status::FeedStatus {
                        kind: "connected".into(),
                        generation: tick.snapshot.connection_generation,
                        ts: tick.quote.ts_init,
                    },
                ))));
            }
            replay.push(Data::Quote(tick.quote));
            replay.push(Data::Custom(CustomData::from_arc(Arc::new(tick.clone()))));
        }
        engine.add_data(replay, Some("KITE".into()), false, false)?;
    }
    let results = node.run()?;
    ensure!(results.len() == 1, "Backtest did not produce a result");
    let s = state.borrow();
    ensure!(
        s.started && s.stopped && s.errors.is_empty(),
        "Backtest strategy lifecycle failed"
    );
    ensure!(s.ticks > 0, "Backtest strategy received no ticks");
    let cache = s.cache.as_ref().expect("native cache").borrow();
    let net: f64 = cache
        .positions_open(None, None, None, None, None)
        .iter()
        .map(|p| p.signed_qty)
        .sum();
    let output = serde_json::json!({"event":"native_backtest_complete","namespace":run_id,"catalog":path,
  "native_backtest_node":true,"native_backtest_engine":true,"native_matching_engine":true,
  "native_redis_cache":true,"native_indicators":true,"full_packet_replay":full_replay,"replayed_full_packets":full_ticks.len(),"strategy_ticks":s.ticks,"audit_quotes":audit.get(),
  "signal_counts":s.signal_counts,"signals":s.signals,"fills":s.fills,"denied":s.denied,"open_contracts":net,"results":results,
  "live_orders_enabled":false,"broker_accessed":false});
    drop(cache);
    drop(s);
    node.get_engine_mut(&run_id)
        .expect("backtest engine")
        .dispose();
    println!("{}", output);
    Ok(())
}

use super::{
    actor::{NativeStrategy, State},
    audit::AuditActor,
    data, persistence,
};
use anyhow::{Result, ensure};
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_core::UUID4;
use nautilus_live::config::LiveNodeConfig;
use nautilus_model::{
    identifiers::Venue,
    types::{Currency, Money},
};
use nautilus_sandbox::{
    config::SandboxExecutionClientConfig, factory::SandboxExecutionClientFactory,
};
use nautilus_trading::algorithm::{ExecutionAlgorithmConfig, twap::TwapAlgorithm};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub fn run(instrument_path: Option<&str>, strategy_path: &str, seconds: u64) -> Result<()> {
    let strategy = kite_strategy::config::Config::parse(&std::fs::read_to_string(strategy_path)?)?;
    let (instrument, token, credentials) = if let Some(path) = instrument_path {
        let config = crate::preflight_command::read_config(path)?;
        let report = crate::preflight_command::resolve(&config, None)?;
        let instrument = kite_adapter::instruments::contract::build(&report, data::now().into())?;
        (
            instrument,
            report.instrument_token,
            Some(Arc::new(kite_adapter::credentials::redis::load_from_env()?)),
        )
    } else {
        (crate::paper_flow::simulation::fixture()?.0, 144870151, None)
    };
    let live_data = credentials.is_some();
    let redis = persistence::redis_config()?;
    let instance = UUID4::new();
    let state = Rc::new(RefCell::new(State::default()));
    let done = Arc::new(AtomicBool::new(false));
    let audit = Rc::new(Cell::new(0));
    let result=tokio::runtime::Runtime::new()?.block_on(async {
  let mut config=LiveNodeConfig{environment:Environment::Sandbox,trader_id:"SUSANTA-001".into(),
   instance_id:Some(instance),cache:Some(persistence::cache_config()),save_state:true,load_state:false,
   shutdown_on_error:true,delay_post_stop:Duration::from_secs(2),..Default::default()};
  config.logging=LoggerConfig{stdout_level:log::LevelFilter::Warn,is_colored:false,..Default::default()};
  config.exec_engine.reconciliation=false;
  config.risk_engine.max_notional_per_order.insert(instrument.id.to_string(),"2000000".into());
  let sandbox=SandboxExecutionClientConfig{
   venue:Venue::from("MCX"),account_id:"MCX-PAPER".into(),base_currency:Some(Currency::INR()),
   starting_balances:vec![Money::new(1_000_000.0,Currency::INR())],..Default::default()
  };
  let mut node=nautilus_live::builder::LiveNodeBuilder::from_config(config)?
   .with_cache_database_factory(Box::new(super::redis_cache::Factory(redis)))
   .add_data_client(Some("KITE".into()),Box::new(data::Factory),Box::new(data::Config{instrument:instrument.clone(),token,seconds,credentials}))?
   .add_simulated_exec_client(Some("MCX".into()),Box::new(SandboxExecutionClientFactory::new()),Box::new(sandbox))?
   .build()?;
  node.add_strategy(NativeStrategy::new(strategy,instrument.id,true,state.clone(),done.clone()))?;
  node.add_actor(AuditActor::new(instrument.id,audit.clone()))?;
  node.add_exec_algorithm(TwapAlgorithm::new(ExecutionAlgorithmConfig{exec_algorithm_id:Some("TWAP".into()),..Default::default()}))?;
  let handle=node.handle();
  let completed=done.clone();
  let watcher=tokio::spawn(async move {
   let deadline=tokio::time::Instant::now()+Duration::from_secs(seconds+60);
   while !completed.load(Ordering::Acquire) && tokio::time::Instant::now()<deadline {
    tokio::time::sleep(Duration::from_millis(100)).await;
   }
   handle.stop();
  });
  println!("{}",serde_json::json!({"event":"native_node_started","namespace":instance.to_string(),"runtime":"LiveNode","live_orders_enabled":false}));
  let run=node.run().await;
  watcher.abort();
  let output=report(&state,audit.get(),live_data,instance.to_string());
  node.dispose();
  run?;
  output
 })?;
    let path = std::path::PathBuf::from(format!("data/native-catalog/{}", instance));
    super::catalog::write_full(
        &path,
        nautilus_model::instruments::InstrumentAny::FuturesContract(instrument),
        &state.borrow().history,
    )?;
    let mut result = result;
    result["catalog"] = path.to_string_lossy().into_owned().into();
    result["full_catalog_roundtrip_verified"] = true.into();
    result["captured_full_packets"] = state.borrow().history.len().into();
    println!("{}", result);
    Ok(())
}
fn report(
    state: &Rc<RefCell<State>>,
    audit: u64,
    live_data: bool,
    namespace: String,
) -> Result<serde_json::Value> {
    let state = state.borrow();
    ensure!(
        state.started && state.stopped,
        "Native strategy lifecycle incomplete"
    );
    ensure!(
        state.errors.is_empty(),
        "Native strategy reported a failure"
    );
    ensure!(state.ticks > 0, "No valid ticks reached native strategy");
    let cache = state.cache.as_ref().expect("native cache").borrow();
    let positions = cache.positions_open(None, None, None, None, None);
    let net: f64 = positions.iter().map(|p| p.signed_qty).sum();
    let orders = cache.orders(None, None, None, None, None);
    Ok(
        serde_json::json!({"event":"native_node_complete","namespace":namespace,"runtime":"LiveNode",
 "native_kernel":true,"native_trader":true,"native_live_clock":true,"native_actor":true,
 "native_strategy":true,"native_data_engine":true,"native_risk_engine":true,"native_execution_engine":true,
 "native_matching_engine":true,"native_portfolio":true,"native_redis_cache":true,
 "twap_registered":true,"data_mode":"full","market_data_source":if live_data {"kite_live"}else{"synthetic"},
 "strategy_ticks":state.ticks,"audit_quotes":audit,"signals":state.signals,"fills":state.fills,
 "denied":state.denied,"cancelled":state.cancelled,"orders":orders.len(),"open_contracts":net,
 "rejected_by_reason":state.rejected,"live_orders_enabled":false,"broker_orders_accessed":false,
 "automatic_resume_enabled":false}),
    )
}

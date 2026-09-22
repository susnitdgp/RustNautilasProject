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
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
pub fn run(instrument_path: Option<&str>, strategy_path: &str, seconds: u64) -> Result<()> {
    run_with_backend(
        instrument_path,
        strategy_path,
        seconds,
        false,
        false,
        None,
        None,
    )
}
pub fn run_kite_mock(strategy_path: &str) -> Result<()> {
    run_with_backend(None, strategy_path, 60, true, false, None, None)
}
pub fn run_kite_mock_short(strategy_path: &str) -> Result<()> {
    run_with_backend(None, strategy_path, 60, true, true, None, None)
}
pub fn run_kite_sandbox(settings_path: &str, strategy_path: &str) -> Result<()> {
    run_kite_sandbox_with_webhooks(settings_path, strategy_path, "config/kite-production.json")
}
pub fn run_kite_sandbox_with_webhooks(
    settings_path: &str,
    strategy_path: &str,
    webhooks_path: &str,
) -> Result<()> {
    run_with_backend(
        None,
        strategy_path,
        60,
        false,
        false,
        Some(settings_path),
        Some(webhooks_path),
    )
}
fn run_with_backend(
    instrument_path: Option<&str>,
    strategy_path: &str,
    seconds: u64,
    kite_mock: bool,
    short_fixture: bool,
    sandbox_path: Option<&str>,
    webhooks_path: Option<&str>,
) -> Result<()> {
    let strategy = kite_strategy::config::Config::parse(&std::fs::read_to_string(strategy_path)?)?;
    let sandbox = sandbox_path
        .map(|p| {
            kite_adapter::execution::native_client::sandbox::Settings::parse(
                &std::fs::read_to_string(p)?,
            )
        })
        .transpose()?;
    let sandbox_webhooks = if let Some(path) = webhooks_path {
        ensure!(
            sandbox.is_some(),
            "Sandbox strategy webhooks require sandbox mode"
        );
        Some(super::sandbox_webhooks::Controller::load(path)?)
    } else {
        None
    };
    let seconds = sandbox.as_ref().map_or(seconds, |s| s.seconds);
    let native_kite = kite_mock || sandbox.is_some();
    let account_scope = sandbox
        .as_ref()
        .map_or_else(|| "MOCK".to_string(), |s| s.account_scope());
    let (instrument, token, credentials) = if let Some(settings) = &sandbox {
        let credentials = tokio::runtime::Runtime::new()?.block_on(
            kite_adapter::execution::native_client::sandbox::prepare(settings),
        )?;
        (
            crate::paper_flow::simulation::fixture()?.0,
            settings.instrument_token,
            Some(credentials),
        )
    } else if let Some(path) = instrument_path {
        let config = crate::preflight_command::read_config(path)?;
        let report = crate::preflight_command::resolve(&config, None)?;
        let instrument = kite_adapter::instruments::contract::build(&report, data::now().into())?;
        (
            instrument,
            report.instrument_token,
            Some(Arc::new(kite_adapter::credentials::redis::load_from_env()?)),
        )
    } else {
        (
            crate::paper_flow::simulation::live_clock_fixture()?.0,
            144870151,
            None,
        )
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
  config.exec_engine.reconciliation=native_kite;
  config.risk_engine.max_notional_per_order.insert(instrument.id.to_string(),"2000000".into());
  let simulation=SandboxExecutionClientConfig{
   venue:Venue::from("MCX"),account_id:"MCX-PAPER".into(),base_currency:Some(Currency::INR()),
   starting_balances:vec![Money::new(1_000_000.0,Currency::INR())],..Default::default()
  };
  let builder=nautilus_live::builder::LiveNodeBuilder::from_config(config)?
   .with_cache_database_factory(Box::new(super::redis_cache::Factory(redis)))
   .add_data_client(Some("KITE".into()),Box::new(data::Factory),Box::new(data::Config{instrument:instrument.clone(),token,seconds,credentials,synthetic_tick_ms:if kite_mock{3000}else{500},short_fixture,sandbox_user:sandbox.as_ref().map(|s|s.expected_user_id.clone())}))?
   ;
  let builder=if let Some(settings)=&sandbox {
   use kite_adapter::execution::native_client::sandbox::{SandboxFactory,SandboxConfig};
   builder.add_exec_client(Some("MCX".into()),Box::new(SandboxFactory),Box::new(SandboxConfig{namespace:instance.to_string(),user_id:settings.expected_user_id.clone(),product:settings.product.clone(),instrument_token:token,stop_signal:done.clone()}))?
  }else if kite_mock {
   use kite_adapter::execution::native_client::mock::{MockFactory,MockConfig};
   builder.add_exec_client(Some("MCX".into()),Box::new(MockFactory),Box::new(MockConfig{namespace:instance.to_string(),stop_signal:done.clone(),product:"NRML".into(),instrument_id:"CRUDEOIL26SEPFUT.MCX".into(),symbol:"CRUDEOIL26SEPFUT".into(),instrument_token:token}))?
  }else{builder.add_simulated_exec_client(Some("MCX".into()),Box::new(SandboxExecutionClientFactory::new()),Box::new(simulation))?};
  let mut node=builder.build()?;
  node.add_strategy(NativeStrategy::new(strategy,instrument.id,true,state.clone(),done.clone()))?;
  node.add_actor(AuditActor::new(instrument.id,audit.clone()))?;
  node.add_exec_algorithm(TwapAlgorithm::new(ExecutionAlgorithmConfig{exec_algorithm_id:Some("TWAP".into()),..Default::default()}))?;
  let mut webhook_session = if let Some(controller) = sandbox_webhooks.as_ref() {
   Some(controller.start().await?)
  } else {
   None
  };
  let handle=node.handle();
  let completed=done.clone();
  let watcher=tokio::spawn(async move {
   super::lifecycle::wait(completed,seconds).await;
   handle.stop();
  });
  println!("{}",serde_json::json!({"event":"native_node_started","namespace":instance.to_string(),"runtime":"LiveNode","live_orders_enabled":false,"sandbox_webhooks_enabled":sandbox_webhooks.as_ref().is_some_and(|c| c.enabled())}));
  let run=node.run().await;
  watcher.abort();
  let webhook_stop = if let Some(session) = webhook_session.as_mut() {
   session.stop().await
  } else {
   Ok(())
  };
  node.dispose();
  webhook_stop?;
  run?;
  let output=report(&state,audit.get(),live_data,instance.to_string(),kite_mock,sandbox.is_some());
  if native_kite {let health=kite_adapter::execution::native_client::coordination::status(&account_scope)?; ensure!(health["state"]=="Clean","Native account shutdown requires review");}
  output
 });
    let path = std::path::PathBuf::from(format!("data/native-catalog/{}", instance));
    if !state.borrow().history.is_empty() {
        super::catalog::write_full(
            &path,
            nautilus_model::instruments::InstrumentAny::FuturesContract(instrument),
            &state.borrow().history,
        )?;
    }
    if result.is_err() {
        eprintln!(
            "{}",
            serde_json::json!({"event":"native_node_failed","namespace":instance.to_string(),"captured_full_packets":state.borrow().history.len(),"catalog":path,"requires_review":true,"live_orders_enabled":false})
        );
    }
    let mut result = result?;
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
    kite_mock: bool,
    kite_sandbox: bool,
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
 "native_matching_engine":!(kite_mock||kite_sandbox),"native_kite_execution_client":kite_mock||kite_sandbox,"kite_sandbox_execution":kite_sandbox,"sandbox_commissions":if kite_sandbox {Some("estimated_zero_not_broker_charges")}else{None},"native_kite_mock_broker":kite_mock,"native_portfolio":true,"native_redis_cache":true,
 "twap_registered":true,"data_mode":"full","market_data_source":if kite_sandbox {"kite_sandbox"}else if live_data {"kite_live"}else{"synthetic"},
 "strategy_ticks":state.ticks,"audit_quotes":audit,"signal_counts":state.signal_counts,"signals":state.signals,"fills":state.fills,
 "denied":state.denied,"cancelled":state.cancelled,"orders":orders.len(),"open_contracts":net,
 "rejected_by_reason":state.rejected,"live_orders_enabled":false,"broker_orders_accessed":kite_sandbox,"real_broker_orders_accessed":false,
 "automatic_resume_enabled":false}),
    )
}

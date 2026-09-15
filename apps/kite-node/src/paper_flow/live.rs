use super::session::Session;
use crate::preflight_command::{read_config, resolve};
use anyhow::{Result, anyhow, ensure};
use kite_adapter::{
    credentials::redis,
    data::{config::KiteDataClientConfig, events::AdapterEvent},
    factories::KiteDataClientFactory,
    instruments::contract,
    websocket::supervisor::FeedEvent,
};
use nautilus_common::{
    cache::CacheView,
    factories::DataClientFactory,
    messages::data::{SubscribeCommand, SubscribeQuotes},
};
use nautilus_core::UUID4;
use nautilus_data::client::DataClientAdapter;
use nautilus_model::identifiers::{ClientId, Venue};
use std::sync::Arc;
use tokio::time::{Duration, Instant};
fn now() -> u64 {
    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default() as u64
}
pub fn run(instrument_path: &str, strategy_path: &str, seconds: u64) -> Result<()> {
    let config = read_config(instrument_path)?;
    let strategy = kite_strategy::config::Config::parse(&std::fs::read_to_string(strategy_path)?)?;
    let credentials = Arc::new(redis::load_from_env()?);
    let report = resolve(&config, None)?;
    let instrument = contract::build(&report, now().into())?;
    let namespace = UUID4::new().to_string();
    println!(
        "{}",
        serde_json::json!({"event":"paper_session_started","namespace":namespace,"live_orders_enabled":false})
    );
    let url = kite_journal::connection::url_from_env()?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let client_config = KiteDataClientConfig {
        instrument: instrument.clone(),
        instrument_token: report.instrument_token,
        duration_seconds: seconds,
        credentials,
        events: tx,
    };
    let result=tokio::runtime::Runtime::new()?.block_on(async {
  let mut session=Session::new(&url,&namespace,strategy.clone(),&instrument,now().into())?;
  session.realtime=true;
  session.expected_token=Some(report.instrument_token);
  let mut client=KiteDataClientFactory.create("KITE",&client_config,CacheView::new(session.core.cache.clone()),session.core.clock.clone())?;
  client.start()?;client.connect().await?;
  session.core.engine.register_client(DataClientAdapter::new(ClientId::new("KITE"),Some(Venue::new("MCX")),false,false,client),Some(Venue::new("MCX")));
  session.core.engine.start();
  let outcome:Result<serde_json::Value>=async{
   session.core.engine.execute_subscribe(SubscribeCommand::Quotes(SubscribeQuotes::new(instrument.id,Some(ClientId::new("KITE")),Some(Venue::new("MCX")),UUID4::new(),now().into(),None,None)))?;
   let deadline=Instant::now()+Duration::from_secs(seconds+15);
   loop{
    let event=tokio::select!{
     e=rx.recv()=>e.ok_or_else(||anyhow!("Kite feed channel closed"))?,
     _=tokio::time::sleep_until(deadline)=>return Err(anyhow!("Kite paper feed timed out")),
     _=tokio::signal::ctrl_c()=>{session.shutdown()?;break;},
     _=tokio::time::sleep(Duration::from_secs(1))=>{
      if session.controls.connected && session.controls.last_source>0 && now().saturating_sub(session.controls.last_source)>u64::from(strategy.max_age_seconds)*1_000_000_000 {session.stale()?;}
      continue;
     }
    };
    match event{
     AdapterEvent::Full{snapshot}=>session.full(*snapshot,now())?,
     AdapterEvent::Feed(FeedEvent::Connected{generation})=>session.connected(u64::from(generation))?,
     AdapterEvent::Feed(FeedEvent::Gap{..})=>session.gap()?,
     AdapterEvent::Feed(FeedEvent::Snapshot(_))=>return Err(anyhow!("Unexpected raw Kite snapshot")),
     AdapterEvent::Failed=>return Err(anyhow!("Kite feed failed; paper session interrupted")),
     AdapterEvent::Complete(summary)=>{ensure!(summary.final_source_fresh,"Kite feed ended without fresh source data");break;}
    }
   }
   let mut result=session.finish()?;
   ensure!(result["quotes"].as_u64().unwrap_or_default()>0,"No live quotes reached native strategy");
   result["market_data_source"]="kite_live".into();
   result["data_mode"]="full".into();
   result["strategy_input"]="KiteFullTick".into();
   ensure!(result["full_ticks"].as_u64().unwrap_or_default()>0,"No full ticks reached native strategy");
   result["instrument_id"]=instrument.id.to_string().into();
   Ok(result)
  }.await;
  if outcome.is_err(){let _=session.shutdown();}
  session.core.engine.stop();
  for client in session.core.engine.get_clients_mut(){client.disconnect().await?;}
  outcome
 })?;
    println!(
        "{}",
        serde_json::json!({"namespace":namespace,"persistence":"redis_aof","result":result})
    );
    Ok(())
}

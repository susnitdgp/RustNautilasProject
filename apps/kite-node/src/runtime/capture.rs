use super::core::Core;
use crate::preflight_command::{read_config, resolve};
use anyhow::{Result, anyhow, ensure};
use chrono::Utc;
use kite_adapter::{
    credentials::redis,
    data::{config::KiteDataClientConfig, events::AdapterEvent},
    factories::KiteDataClientFactory,
    instruments::contract,
    websocket::supervisor::FeedEvent,
};
use kite_recorder::{records::Record, writer::Recorder};
use nautilus_common::{
    cache::CacheView,
    factories::DataClientFactory,
    messages::data::{SubscribeCommand, SubscribeQuotes},
};
use nautilus_core::UUID4;
use nautilus_data::client::DataClientAdapter;
use nautilus_model::identifiers::{ClientId, Venue};
use std::{path::Path, sync::Arc};
use tokio::time::{Duration, Instant, timeout_at};

pub fn run(config_path: &str, seconds: u64, output: &Path) -> Result<()> {
    let config = read_config(config_path)?;
    let credentials = Arc::new(redis::load_from_env()?);
    let report = resolve(&config, None)?;
    let now = u64::try_from(
        Utc::now()
            .timestamp_nanos_opt()
            .ok_or_else(|| anyhow!("Clock overflow"))?,
    )?
    .into();
    let instrument = contract::build(&report, now)?;
    let recorder = Recorder::create(output)?;
    recorder.record(Record::Header {
        schema_version: 1,
        instrument: Box::new(instrument.clone()),
        instrument_token: report.instrument_token,
    })?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let client_config = KiteDataClientConfig {
        instrument: instrument.clone(),
        instrument_token: report.instrument_token,
        duration_seconds: seconds,
        credentials,
        events: tx,
    };
    let result = tokio::runtime::Runtime::new()?.block_on(async {
        let mut core = Core::new(&instrument);
        let mut client = KiteDataClientFactory.create("KITE", &client_config,
            CacheView::new(core.cache.clone()), core.clock.clone())?;
        client.start()?;
        client.connect().await?;
        core.engine.register_client(DataClientAdapter::new(ClientId::new("KITE"),
            Some(Venue::new("MCX")), false, false, client), Some(Venue::new("MCX")));
        core.engine.start();
        core.engine.execute_subscribe(SubscribeCommand::Quotes(SubscribeQuotes::new(instrument.id,
            Some(ClientId::new("KITE")), Some(Venue::new("MCX")), UUID4::new(), now, None, None)))?;
        let deadline = Instant::now() + Duration::from_secs(seconds+15);
        let mut accepted = 0u64;
        let mut gaps = 0u64;
        let outcome: Result<_> = async {
            loop {
                let event = tokio::select! {
                    result = timeout_at(deadline, rx.recv()) => result
                        .map_err(|_| anyhow!("Data client completion timed out"))?
                        .ok_or_else(|| anyhow!("Data client channel closed unexpectedly"))?,
                    _ = tokio::signal::ctrl_c() => return Err(anyhow!("Capture interrupted; file will be incomplete")),
                };
                match event {
                    AdapterEvent::Quote { quote, generation } => {
                        recorder.record(Record::Quote { quote, generation })?;
                        core.quote(quote); accepted += 1;
                    }
                    AdapterEvent::Feed(FeedEvent::Connected {generation}) => recorder.record(Record::Connected {generation})?,
                    AdapterEvent::Feed(FeedEvent::Gap {generation}) => { recorder.record(Record::Gap {generation})?; gaps += 1; }
                    AdapterEvent::Feed(FeedEvent::Snapshot(_)) => return Err(anyhow!("Unexpected raw snapshot in data client")),
                    AdapterEvent::Failed => return Err(anyhow!("Data client failed: transport, mapping or queue overflow; capture incomplete")),
                    AdapterEvent::Complete(summary) => {
                        ensure!(accepted > 0 && summary.final_source_fresh, "No fresh normalized quotes; capture verification incomplete");
                        ensure!(core.callbacks() == accepted, "DataEngine callback count mismatch");
                        let cached = core.cache.borrow().quote(&instrument.id).copied();
                        ensure!(cached == core.last_quote, "DataEngine cache differs from final quote");
                        recorder.record(Record::End {quotes:accepted})?;
                        break Ok((accepted, gaps, core.callbacks()));
                    }
                }
            }
        }.await;
        core.engine.stop();
        for client in core.engine.get_clients_mut() { client.disconnect().await?; }
        outcome
    });
    let rows = recorder.finish()?;
    let (quotes, gaps, callbacks) = result?;
    println!(
        "{}",
        serde_json::json!({
            "event":"capture_complete", "instrument_id":instrument.id.to_string(),
            "data_engine_callbacks":callbacks, "recorded_quotes":quotes,
            "records":rows, "gaps":gaps, "live_orders_enabled":false,
        })
    );
    Ok(())
}

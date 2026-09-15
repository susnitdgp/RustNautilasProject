//! Native LiveNode DataClient: emits on the runner's data channel.
use super::status::FeedStatus;
use anyhow::{Result, anyhow, ensure};
use async_trait::async_trait;
use kite_adapter::{
    credentials::KiteCredentials,
    data::full_tick::KiteFullTick,
    mapping::{market_data::Snapshot, quotes},
    websocket::{
        supervisor::{self, FeedEvent},
        transport,
    },
};
use nautilus_common::{
    cache::CacheView,
    clients::DataClient,
    clock::Clock,
    factories::{ClientConfig, DataClientFactory},
    live::get_data_event_sender,
    messages::{
        DataEvent,
        data::{SubscribeCustomData, SubscribeQuotes, UnsubscribeCustomData, UnsubscribeQuotes},
    },
};
use nautilus_model::{
    data::{CustomData, Data},
    identifiers::{ClientId, Venue},
    instruments::{FuturesContract, InstrumentAny},
};
use std::{any::Any, cell::RefCell, rc::Rc, sync::Arc};
use tokio::{
    task::JoinHandle,
    time::{Duration, Instant},
};
#[derive(Debug, Clone)]
pub struct Config {
    pub instrument: FuturesContract,
    pub token: u32,
    pub seconds: u64,
    pub synthetic_tick_ms: u64,
    pub short_fixture: bool,
    pub sandbox_user: Option<String>,
    pub credentials: Option<Arc<KiteCredentials>>,
}
impl ClientConfig for Config {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
#[derive(Debug)]
pub struct Factory;
impl DataClientFactory for Factory {
    fn name(&self) -> &str {
        "KITE"
    }
    fn config_type(&self) -> &str {
        "NativeKiteDataConfig"
    }
    fn create(
        &self,
        _: &str,
        config: &dyn ClientConfig,
        _: CacheView,
        _: Rc<RefCell<dyn Clock>>,
    ) -> Result<Box<dyn DataClient>> {
        let config = config
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| anyhow!("Invalid native Kite config"))?
            .clone();
        Ok(Box::new(Client {
            config,
            connected: false,
            task: None,
            socket: None,
        }))
    }
}
struct Client {
    config: Config,
    connected: bool,
    task: Option<JoinHandle<()>>,
    socket: Option<transport::Socket>,
}
pub fn now() -> u64 {
    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default() as u64
}
fn status(tx: &tokio::sync::mpsc::UnboundedSender<DataEvent>, kind: &str, generation: u32) {
    let event = FeedStatus {
        kind: kind.into(),
        generation,
        ts: now().into(),
    };
    let _ = tx.send(DataEvent::Data(Data::Custom(CustomData::from_arc(
        Arc::new(event),
    ))));
}
fn emit(
    tx: &tokio::sync::mpsc::UnboundedSender<DataEvent>,
    snapshot: Snapshot,
    instrument: &FuturesContract,
) {
    let generation = snapshot.connection_generation;
    if !snapshot
        .raw
        .as_ref()
        .is_some_and(|r| r.full.is_some() && r.quote_fields.is_some())
    {
        status(tx, "incomplete_full_packet", generation);
        return;
    }
    if !snapshot.source_fresh {
        status(tx, "stale_packet", generation);
        return;
    }
    match quotes::map(&snapshot, instrument) {
        Ok(Some(q))
            if q.bid_price > nautilus_model::types::Price::new(0.0, 0)
                && q.ask_price >= q.bid_price =>
        {
            let _ = tx.send(DataEvent::Data(Data::Quote(q)));
            let _ = tx.send(DataEvent::Data(Data::Custom(CustomData::from_arc(
                Arc::new(KiteFullTick { snapshot, quote: q }),
            ))));
        }
        _ => status(tx, "invalid_top_quote", generation),
    }
}
impl Client {
    fn begin(&mut self) -> Result<()> {
        if self.task.is_some() {
            return Ok(());
        }
        ensure!(self.connected, "Kite client disconnected");
        let config = self.config.clone();
        let socket = self.socket.take();
        let tx = get_data_event_sender();
        self.task = Some(tokio::spawn(async move {
            if let Some(credentials) = config.credentials {
                let on_event = |e| match e {
                    FeedEvent::Connected { generation } => status(&tx, "connected", generation),
                    FeedEvent::Gap { generation } => status(&tx, "gap", generation),
                    FeedEvent::Snapshot(s) => emit(&tx, *s, &config.instrument),
                };
                let socket = socket.expect("live socket connected");
                let outcome = if let Some(user) = config.sandbox_user {
                    supervisor::observe_sandbox_connected(
                        &credentials,
                        &user,
                        config.token,
                        Duration::from_secs(config.seconds),
                        on_event,
                        socket,
                    )
                    .await
                } else {
                    supervisor::observe_connected(
                        &credentials,
                        config.token,
                        Duration::from_secs(config.seconds),
                        on_event,
                        socket,
                    )
                    .await
                };
                status(
                    &tx,
                    if outcome.is_ok_and(|s| s.final_source_fresh) {
                        "complete"
                    } else {
                        "failed"
                    },
                    0,
                );
            } else {
                status(&tx, "connected", 1);
                for p in [
                    6008, 6006, 6004, 6002, 6000, 6002, 6004, 6006, 6005, 6005, 6003, 6003, 6001,
                    5999, 6000,
                ] {
                    tokio::time::sleep(Duration::from_millis(config.synthetic_tick_ms)).await;
                    emit(
                        &tx,
                        crate::paper_flow::simulation::full_snapshot(
                            if config.short_fixture { 12000 - p } else { p },
                            now(),
                            1,
                        ),
                        &config.instrument,
                    );
                }
                status(&tx, "complete", 1);
            }
        }));
        Ok(())
    }
    fn cancel(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
        self.socket = None;
        self.connected = false;
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.cancel();
    }
}
#[async_trait(?Send)]
impl DataClient for Client {
    fn client_id(&self) -> ClientId {
        ClientId::from("KITE")
    }
    fn venue(&self) -> Option<Venue> {
        Some(Venue::from("MCX"))
    }
    fn start(&mut self) -> Result<()> {
        Ok(())
    }
    fn stop(&mut self) -> Result<()> {
        self.cancel();
        Ok(())
    }
    fn reset(&mut self) -> Result<()> {
        self.cancel();
        Ok(())
    }
    fn dispose(&mut self) -> Result<()> {
        self.cancel();
        Ok(())
    }
    fn is_connected(&self) -> bool {
        self.connected
    }
    fn is_disconnected(&self) -> bool {
        !self.connected
    }
    async fn connect(&mut self) -> Result<()> {
        if let Some(credentials) = &self.config.credentials {
            self.socket = Some(if let Some(user) = &self.config.sandbox_user {
                transport::connect_sandbox(
                    credentials,
                    user,
                    Instant::now() + Duration::from_secs(10),
                )
                .await?
            } else {
                kite_adapter::auth::session::validate(credentials, "MCX").await?;
                transport::connect(credentials, Instant::now() + Duration::from_secs(10)).await?
            });
        }
        get_data_event_sender()
            .send(DataEvent::Instrument(InstrumentAny::FuturesContract(
                self.config.instrument.clone(),
            )))
            .map_err(|_| anyhow!("Native data channel closed"))?;
        self.connected = true;
        Ok(())
    }
    async fn disconnect(&mut self) -> Result<()> {
        self.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        Ok(())
    }
    fn subscribe(&mut self, cmd: SubscribeCustomData) -> Result<()> {
        match cmd.data_type.type_name() {
            "KiteFullTick" => self.begin(),
            "KiteFeedStatus" => Ok(()),
            _ => Err(anyhow!("Unsupported native Kite custom subscription")),
        }
    }
    fn unsubscribe(&mut self, _: &UnsubscribeCustomData) -> Result<()> {
        Ok(())
    }
    fn subscribe_quotes(&mut self, cmd: SubscribeQuotes) -> Result<()> {
        ensure!(
            cmd.instrument_id == self.config.instrument.id,
            "Wrong quote instrument"
        );
        self.begin()
    }
    fn unsubscribe_quotes(&mut self, _: &UnsubscribeQuotes) -> Result<()> {
        Ok(())
    }
}

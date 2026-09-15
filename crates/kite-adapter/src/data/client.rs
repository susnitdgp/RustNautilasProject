use super::{config::KiteDataClientConfig, events::AdapterEvent};
use crate::{
    auth::session,
    websocket::{
        supervisor::{self, FeedEvent},
        transport::{self, Socket},
    },
};
use anyhow::{Result, anyhow, ensure};
use async_trait::async_trait;
use nautilus_common::{cache::CacheView, clients::DataClient, messages::data::*};
use nautilus_model::identifiers::{ClientId, Venue};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::{
    sync::Notify,
    task::JoinHandle,
    time::{Duration, Instant},
};

pub struct KiteDataClient {
    id: ClientId,
    config: KiteDataClientConfig,
    cache: CacheView,
    socket: Option<Socket>,
    worker: Option<JoinHandle<()>>,
    connected: Arc<AtomicBool>,
    started: bool,
}
impl KiteDataClient {
    pub fn new(name: &str, config: KiteDataClientConfig, cache: CacheView) -> Result<Self> {
        ensure!(name == "KITE", "Client name must be KITE");
        ensure!(
            config.instrument.id.to_string() == "CRUDEOIL26SEPFUT.MCX",
            "Unsupported contract"
        );
        ensure!(
            config.instrument_token > 0 && (1..=300).contains(&config.duration_seconds),
            "Invalid data client configuration"
        );
        Ok(Self {
            id: ClientId::new(name),
            config,
            cache,
            socket: None,
            worker: None,
            connected: Arc::new(AtomicBool::new(false)),
            started: false,
        })
    }
    fn cancel(&mut self) {
        if let Some(worker) = &self.worker {
            worker.abort();
        }
        self.socket = None;
        self.connected.store(false, Ordering::Release);
    }
}
impl Drop for KiteDataClient {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[async_trait(?Send)]
impl DataClient for KiteDataClient {
    fn client_id(&self) -> ClientId {
        self.id
    }
    fn venue(&self) -> Option<Venue> {
        Some(Venue::new("MCX"))
    }
    fn start(&mut self) -> Result<()> {
        self.started = true;
        Ok(())
    }
    fn stop(&mut self) -> Result<()> {
        self.cancel();
        self.started = false;
        Ok(())
    }
    fn reset(&mut self) -> Result<()> {
        ensure!(
            self.worker.is_none(),
            "Disconnect and drain the worker before reset"
        );
        self.socket = None;
        self.started = false;
        self.connected.store(false, Ordering::Release);
        Ok(())
    }
    fn dispose(&mut self) -> Result<()> {
        self.stop()
    }
    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
    fn is_disconnected(&self) -> bool {
        !self.is_connected()
    }
    async fn connect(&mut self) -> Result<()> {
        ensure!(self.started, "Start client before connecting");
        ensure!(
            self.worker.is_none() && self.socket.is_none(),
            "Client already has a connection or worker"
        );
        ensure!(
            self.cache
                .borrow()
                .instrument(&self.config.instrument.id)
                .is_some(),
            "Instrument must be observable in Nautilus cache before connecting"
        );
        session::validate(&self.config.credentials, "MCX").await?;
        self.socket = Some(
            transport::connect_at(
                &self.config.credentials,
                Instant::now() + Duration::from_secs(10),
                "wss://ws.kite.trade",
            )
            .await?,
        );
        self.connected.store(true, Ordering::Release);
        Ok(())
    }
    async fn disconnect(&mut self) -> Result<()> {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.await;
        }
        Ok(())
    }
    fn subscribe_quotes(&mut self, cmd: SubscribeQuotes) -> Result<()> {
        ensure!(
            cmd.instrument_id == self.config.instrument.id,
            "Unsupported quote instrument"
        );
        ensure!(
            self.started && self.is_connected(),
            "Client is not connected"
        );
        if self.worker.is_some() {
            return Ok(());
        }
        let socket = self
            .socket
            .take()
            .ok_or_else(|| anyhow!("WebSocket is unavailable"))?;
        let config = self.config.clone();
        let connected = self.connected.clone();
        self.worker = Some(tokio::spawn(async move {
            let failed = Arc::new(Notify::new());
            let signal = failed.clone();
            let run = supervisor::observe_connected(
                &config.credentials,
                config.instrument_token,
                Duration::from_secs(config.duration_seconds),
                |event| {
                    let event = match event {
                        FeedEvent::Snapshot(snapshot) => AdapterEvent::Full { snapshot },
                        other => {
                            connected.store(
                                matches!(other, FeedEvent::Connected { .. }),
                                Ordering::Release,
                            );
                            AdapterEvent::Feed(other)
                        }
                    };
                    if config.events.try_send(event).is_err() {
                        signal.notify_one();
                    }
                },
                socket,
            );
            let outcome = tokio::select! {
                biased;
                _ = failed.notified() => None,
                result = run => result.ok(),
            };
            connected.store(false, Ordering::Release);
            let event = match outcome {
                Some(summary) => AdapterEvent::Complete(summary),
                None => AdapterEvent::Failed,
            };
            let _ = config.events.send(event).await;
        }));
        Ok(())
    }
    fn unsubscribe_quotes(&mut self, cmd: &UnsubscribeQuotes) -> Result<()> {
        ensure!(
            cmd.instrument_id == self.config.instrument.id,
            "Unsupported quote instrument"
        );
        self.cancel();
        Ok(())
    }
    // Remaining data capabilities explicitly fail instead of inheriting no-op defaults.
    fn subscribe(&mut self, _cmd: SubscribeCustomData) -> Result<()> {
        Err(anyhow!("Unsupported Kite data capability: subscribe"))
    }
    fn subscribe_instruments(&mut self, _cmd: SubscribeInstruments) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: subscribe_instruments"
        ))
    }
    fn subscribe_instrument(&mut self, _cmd: SubscribeInstrument) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: subscribe_instrument"
        ))
    }
    fn subscribe_book_deltas(&mut self, _cmd: SubscribeBookDeltas) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: subscribe_book_deltas"
        ))
    }
    fn subscribe_book_depth10(&mut self, _cmd: SubscribeBookDepth10) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: subscribe_book_depth10"
        ))
    }
    fn subscribe_trades(&mut self, _cmd: SubscribeTrades) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: subscribe_trades"
        ))
    }
    fn subscribe_mark_prices(&mut self, _cmd: SubscribeMarkPrices) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: subscribe_mark_prices"
        ))
    }
    fn subscribe_index_prices(&mut self, _cmd: SubscribeIndexPrices) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: subscribe_index_prices"
        ))
    }
    fn subscribe_funding_rates(&mut self, _cmd: SubscribeFundingRates) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: subscribe_funding_rates"
        ))
    }
    fn subscribe_bars(&mut self, _cmd: SubscribeBars) -> Result<()> {
        Err(anyhow!("Unsupported Kite data capability: subscribe_bars"))
    }
    fn subscribe_instrument_status(&mut self, _cmd: SubscribeInstrumentStatus) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: subscribe_instrument_status"
        ))
    }
    fn subscribe_instrument_close(&mut self, _cmd: SubscribeInstrumentClose) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: subscribe_instrument_close"
        ))
    }
    fn subscribe_option_greeks(&mut self, _cmd: SubscribeOptionGreeks) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: subscribe_option_greeks"
        ))
    }
    fn unsubscribe(&mut self, _cmd: &UnsubscribeCustomData) -> Result<()> {
        Err(anyhow!("Unsupported Kite data capability: unsubscribe"))
    }
    fn unsubscribe_instruments(&mut self, _cmd: &UnsubscribeInstruments) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_instruments"
        ))
    }
    fn unsubscribe_instrument(&mut self, _cmd: &UnsubscribeInstrument) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_instrument"
        ))
    }
    fn unsubscribe_book_deltas(&mut self, _cmd: &UnsubscribeBookDeltas) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_book_deltas"
        ))
    }
    fn unsubscribe_book_depth10(&mut self, _cmd: &UnsubscribeBookDepth10) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_book_depth10"
        ))
    }
    fn unsubscribe_trades(&mut self, _cmd: &UnsubscribeTrades) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_trades"
        ))
    }
    fn unsubscribe_mark_prices(&mut self, _cmd: &UnsubscribeMarkPrices) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_mark_prices"
        ))
    }
    fn unsubscribe_index_prices(&mut self, _cmd: &UnsubscribeIndexPrices) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_index_prices"
        ))
    }
    fn unsubscribe_funding_rates(&mut self, _cmd: &UnsubscribeFundingRates) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_funding_rates"
        ))
    }
    fn unsubscribe_bars(&mut self, _cmd: &UnsubscribeBars) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_bars"
        ))
    }
    fn unsubscribe_instrument_status(&mut self, _cmd: &UnsubscribeInstrumentStatus) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_instrument_status"
        ))
    }
    fn unsubscribe_instrument_close(&mut self, _cmd: &UnsubscribeInstrumentClose) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_instrument_close"
        ))
    }
    fn unsubscribe_option_greeks(&mut self, _cmd: &UnsubscribeOptionGreeks) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: unsubscribe_option_greeks"
        ))
    }
    fn request_data(&self, _request: RequestCustomData) -> Result<()> {
        Err(anyhow!("Unsupported Kite data capability: request_data"))
    }
    fn request_instruments(&self, _request: RequestInstruments) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: request_instruments"
        ))
    }
    fn request_instrument(&self, _request: RequestInstrument) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: request_instrument"
        ))
    }
    fn request_book_snapshot(&self, _request: RequestBookSnapshot) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: request_book_snapshot"
        ))
    }
    fn request_quotes(&self, _request: RequestQuotes) -> Result<()> {
        Err(anyhow!("Unsupported Kite data capability: request_quotes"))
    }
    fn request_trades(&self, _request: RequestTrades) -> Result<()> {
        Err(anyhow!("Unsupported Kite data capability: request_trades"))
    }
    fn request_funding_rates(&self, _request: RequestFundingRates) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: request_funding_rates"
        ))
    }
    fn request_forward_prices(&self, _request: RequestForwardPrices) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: request_forward_prices"
        ))
    }
    fn request_bars(&self, _request: RequestBars) -> Result<()> {
        Err(anyhow!("Unsupported Kite data capability: request_bars"))
    }
    fn request_book_depth(&self, _request: RequestBookDepth) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: request_book_depth"
        ))
    }
    fn request_book_deltas(&self, _request: RequestBookDeltas) -> Result<()> {
        Err(anyhow!(
            "Unsupported Kite data capability: request_book_deltas"
        ))
    }
}

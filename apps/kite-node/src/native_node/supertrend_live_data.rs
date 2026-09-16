//! Native external-bar client for paper Supertrend; broker reads only.
use super::{data::now, supertrend_live_bars as bars, supertrend_live_control::Control};
use anyhow::{Result, anyhow, ensure};
use async_trait::async_trait;
use kite_adapter::http::historical::{self, Candle};
use nautilus_common::{
    cache::CacheView,
    clients::DataClient,
    clock::Clock,
    factories::{ClientConfig, DataClientFactory},
    live::get_data_event_sender,
    messages::{
        DataEvent,
        data::{SubscribeBars, SubscribeQuotes, UnsubscribeBars, UnsubscribeQuotes},
    },
};
use nautilus_model::{
    data::{BarType, Data, QuoteTick},
    identifiers::{ClientId, Venue},
    instruments::{FuturesContract, InstrumentAny},
    types::Price,
};
use std::{any::Any, cell::RefCell, rc::Rc, sync::atomic::Ordering};
use tokio::task::JoinHandle;
#[derive(Debug, Clone)]
pub struct Config {
    pub instrument: FuturesContract,
    pub token: u32,
    pub date: chrono::NaiveDate,
    pub warmup: Vec<Candle>,
    pub simulated: Vec<Candle>,
    pub control: Control,
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
        "STBARS"
    }
    fn config_type(&self) -> &str {
        "SupertrendBars"
    }
    fn create(
        &self,
        _: &str,
        c: &dyn ClientConfig,
        _: CacheView,
        _: Rc<RefCell<dyn Clock>>,
    ) -> Result<Box<dyn DataClient>> {
        let config = c
            .as_any()
            .downcast_ref::<Config>()
            .ok_or_else(|| anyhow!("Invalid bar config"))?
            .clone();
        Ok(Box::new(Client {
            config,
            connected: false,
            task: None,
        }))
    }
}
struct Client {
    config: Config,
    connected: bool,
    task: Option<JoinHandle<()>>,
}
impl Client {
    fn begin(&mut self) -> Result<()> {
        if self.task.is_some() {
            return Ok(());
        }
        ensure!(self.connected, "Bar client disconnected");
        let c = self.config.clone();
        let tx = get_data_event_sender();
        self.task = Some(tokio::spawn(async move {
            let result = async {
                let bt: BarType = format!("{}-5-MINUTE-LAST-EXTERNAL", c.instrument.id).parse()?;
                let emit = |v: Data| -> Result<()> {
                    tx.send(DataEvent::Data(v))
                        .map_err(|_| anyhow!("Native bar channel closed"))
                };
                let quote = |price: f64, _ts: u64| -> Data {
                    let ts = now();
                    Data::Quote(QuoteTick::new(
                        c.instrument.id,
                        Price::new(price, 0),
                        Price::new(price, 0),
                        1000.into(),
                        1000.into(),
                        ts.into(),
                        now().into(),
                    ))
                };
                for bar in &c.warmup {
                    emit(Data::Bar(bars::bar(bar, bt, now())?))?;
                }
                if c.control.sim {
                    for bar in &c.simulated {
                        if c.control.stopping.load(Ordering::Acquire) {
                            break;
                        }
                        let open = bars::close(bar)? - 300_000_000_000;
                        emit(quote(bar.open, open + 2))?;
                        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                        emit(quote(bar.open, open + 3))?;
                        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                        emit(Data::Bar(bars::bar(bar, bt, now())?))?;
                        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                    }
                    c.control.stop();
                    let last = c
                        .simulated
                        .last()
                        .ok_or_else(|| anyhow!("No simulation bars"))?;
                    for _ in 0..30 {
                        emit(quote(last.close, bars::close(last)? + 2))?;
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    return Ok::<_, anyhow::Error>(());
                }
                let mut tracker = bars::Tracker::new(&c.warmup)?;
                while !c.control.stopping.load(Ordering::Acquire) {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    if c.control.stopping.load(Ordering::Acquire) {
                        break;
                    }
                    let candles = historical::fetch_window(c.token, c.date, 1).await?;
                    let completed = bars::completed(candles, now())?;
                    for bar in tracker.append(completed)? {
                        emit(Data::Bar(bars::bar(&bar, bt, now())?))?;
                    }
                }
                Ok(())
            }
            .await;
            if let Err(e) = result {
                c.control.fail(&format!("Bar feed: {e}"));
            }
        }));
        Ok(())
    }
    fn cancel(&mut self) {
        if let Some(t) = &self.task {
            t.abort();
        }
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
        "STBARS".into()
    }
    fn venue(&self) -> Option<Venue> {
        None
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
        get_data_event_sender()
            .send(DataEvent::Instrument(InstrumentAny::FuturesContract(
                self.config.instrument.clone(),
            )))
            .map_err(|_| anyhow!("Native channel closed"))?;
        self.connected = true;
        Ok(())
    }
    async fn disconnect(&mut self) -> Result<()> {
        self.cancel();
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
        Ok(())
    }
    fn subscribe_bars(&mut self, cmd: SubscribeBars) -> Result<()> {
        let bt: BarType =
            format!("{}-5-MINUTE-LAST-EXTERNAL", self.config.instrument.id).parse()?;
        ensure!(cmd.bar_type == bt, "Wrong live bar type");
        self.begin()
    }
    fn unsubscribe_bars(&mut self, _: &UnsubscribeBars) -> Result<()> {
        Ok(())
    }
    fn subscribe_quotes(&mut self, cmd: SubscribeQuotes) -> Result<()> {
        ensure!(
            self.config.control.sim && cmd.instrument_id == self.config.instrument.id,
            "Quotes must use KITE in paper mode"
        );
        self.begin()
    }
    fn unsubscribe_quotes(&mut self, _: &UnsubscribeQuotes) -> Result<()> {
        Ok(())
    }
}

//! Native external-bar client for paper Supertrend; broker reads only.
use super::{
    data::now, supertrend_bar_timing as timing, supertrend_live_bars as bars,
    supertrend_live_control::Control,
};
use anyhow::{Result, anyhow, ensure};
use async_trait::async_trait;
use kite_adapter::http::historical::{self, Candle, Interval};
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
/// Select against a single request-start cutoff, not a later response time.
/// No history mutation occurs until every expected completed candle validates.
fn prepare_update(
    history: &mut super::supertrend_revision::History,
    candles: Vec<Candle>,
    requested_at: u64,
    date: chrono::NaiveDate,
    interval: Interval,
) -> Result<Option<super::supertrend_revision::Update>> {
    let completed = bars::completed_for(candles, requested_at, interval)?;
    let latest = bars::close_for(completed.last().expect("completed bars"), interval)?;
    if timing::publication_pending(latest, requested_at, interval.nanoseconds()) {
        return Ok(None);
    }
    history.update(completed, date, requested_at).map(Some)
}

#[derive(Debug, Clone)]
pub struct Config {
    pub instrument: FuturesContract,
    pub token: u32,
    pub synthetic_delay_ms: u64,
    pub date: chrono::NaiveDate,
    pub calendar: super::session_calendar::Calendar,
    pub interval: Interval,
    pub volume_sensitive: bool,
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
                let bt: BarType = format!(
                    "{}-{}-MINUTE-LAST-EXTERNAL",
                    c.instrument.id,
                    c.interval.minutes()
                )
                .parse()?;
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
                    emit(Data::Bar(bars::bar_for(bar, bt, now(), c.interval)?))?;
                }
                if c.control.sim {
                    for (index, bar) in c.simulated.iter().enumerate() {
                        if c.control.stopping.load(Ordering::Acquire) {
                            break;
                        }
                        let open = bars::close_for(bar, c.interval)? - c.interval.nanoseconds();
                        emit(quote(bar.open, open + 2))?;
                        tokio::time::sleep(std::time::Duration::from_millis(c.synthetic_delay_ms))
                            .await;
                        emit(quote(bar.open, open + 3))?;
                        tokio::time::sleep(std::time::Duration::from_millis(c.synthetic_delay_ms))
                            .await;
                        if c.control.recovery_fixture && index == 10 {
                            let mut corrected = c.warmup.clone();
                            corrected.extend_from_slice(&c.simulated[..=index]);
                            corrected[50].volume += 1;
                            let epoch = c.control.pause();
                            let replay = corrected
                                .iter()
                                .map(|b| bars::bar_for(b, bt, now(), c.interval))
                                .collect::<Result<Vec<_>>>()?;
                            *c.control.rebuild.lock().expect("rebuild lock") =
                                Some((epoch, replay));
                        } else {
                            emit(Data::Bar(bars::bar_for(bar, bt, now(), c.interval)?))?;
                        }

                        tokio::time::sleep(std::time::Duration::from_millis(c.synthetic_delay_ms))
                            .await;
                    }
                    c.control.stop();
                    let last = c
                        .simulated
                        .last()
                        .ok_or_else(|| anyhow!("No simulation bars"))?;
                    for _ in 0..30 {
                        emit(quote(last.close, bars::close_for(last, c.interval)? + 2))?;
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    return Ok::<_, anyhow::Error>(());
                }
                let mut history = super::supertrend_revision::History::new_for(
                    &c.warmup,
                    c.calendar.clone(),
                    c.interval,
                )?
                .with_volume_sensitive(c.volume_sensitive);
                let step = c.interval.nanoseconds();
                let mut reader = historical::Reader::default();
                let mut next_fetch = timing::next_poll(now(), history.latest_close(), step);
                {
                    let mut stats = c.control.bar_feed.lock().expect("bar feed stats");
                    stats.last_candle_close_ns = history.latest_close();
                    stats.last_candle_open_ns = history.latest_close().saturating_sub(step);
                }
                let mut failures = 0;
                while !c.control.stopping.load(Ordering::Acquire) {
                    let remaining = next_fetch.saturating_sub(now());
                    if remaining > 0 {
                        // Short sleeps keep shutdown responsive without polling the API.
                        tokio::time::sleep(std::time::Duration::from_nanos(
                            remaining.min(250_000_000),
                        ))
                        .await;
                        continue;
                    }
                    let epoch = c.control.epoch.load(Ordering::Acquire);
                    let requested_at = now();
                    let started = std::time::Instant::now();
                    c.control.bar_feed.lock().expect("bar feed stats").fetches += 1;
                    let fetched = reader
                        .fetch_window_for(c.token, c.date, 7, c.interval)
                        .await;
                    let received_at = now();
                    c.control
                        .bar_feed
                        .lock()
                        .expect("bar feed stats")
                        .last_fetch_ms =
                        started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                    if c.control.stopping.load(Ordering::Acquire) {
                        break;
                    }
                    let update = fetched.and_then(|candles| {
                        prepare_update(&mut history, candles, requested_at, c.date, c.interval)
                    });
                    let update = match update {
                        Ok(Some(v)) => {
                            failures = 0;
                            v
                        }
                        Ok(None) => {
                            c.control
                                .bar_feed
                                .lock()
                                .expect("bar feed stats")
                                .publication_waits += 1;
                            next_fetch = received_at.saturating_add(timing::PUBLICATION_RETRY_NS);
                            continue;
                        }
                        Err(e) => {
                            failures += 1;
                            c.control.bar_feed.lock().expect("bar feed stats").failures += 1;
                            c.control.pause();
                            // Never fast-retry failed HTTP requests, authentication,
                            // rate limits or invalid/gapped historical data.
                            if historical::terminal_read_error(&e) || failures >= 6 {
                                return Err(e);
                            }
                            next_fetch = received_at.saturating_add(timing::AUDIT_INTERVAL_NS);
                            continue;
                        }
                    };
                    next_fetch = timing::next_poll(now(), history.latest_close(), step);
                    {
                        let mut stats = c.control.bar_feed.lock().expect("bar feed stats");
                        stats.price_revisions += update.price_revised as u64;
                        stats.volume_only_revisions += update.volume_only_revised as u64;
                        if !c.volume_sensitive {
                            stats.ignored_volume_revisions += update.volume_only_revised as u64;
                        }
                        if !update.revision_samples.is_empty() {
                            stats.last_revision_samples = update.revision_samples.clone();
                        }
                        stats.received(history.latest_close(), step, received_at);
                    }
                    if c.control.epoch.load(Ordering::Acquire) != epoch {
                        next_fetch = now().saturating_add(timing::PUBLICATION_RETRY_NS);
                        continue;
                    }
                    if update.revised > 0 || c.control.paused.load(Ordering::Acquire) {
                        {
                            let mut stats = c.control.bar_feed.lock().expect("bar feed stats");
                            stats.rebuild_requests += 1;
                            stats.last_rebuild_reason = if update.revised > 0 {
                                format!(
                                    "history correction: {} price / {} volume-only",
                                    update.price_revised, update.volume_only_revised
                                )
                            } else {
                                "feed recovery after pause".into()
                            };
                        }
                        let epoch = c.control.pause();
                        let rebuilt = update
                            .all
                            .iter()
                            .map(|b| bars::bar_for(b, bt, received_at, c.interval))
                            .collect::<Result<Vec<_>>>()?;
                        *c.control.rebuild.lock().expect("rebuild lock") = Some((epoch, rebuilt));
                    } else {
                        for bar in update.new {
                            emit(Data::Bar(bars::bar_for(&bar, bt, received_at, c.interval)?))?;
                        }
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
        let bt: BarType = format!(
            "{}-{}-MINUTE-LAST-EXTERNAL",
            self.config.instrument.id,
            self.config.interval.minutes()
        )
        .parse()?;
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

#[cfg(test)]
#[path = "supertrend_live_data_tests.rs"]
mod polling_tests;

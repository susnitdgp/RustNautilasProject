//! Native external-bar client for the active strategy; broker reads only.
use super::{bar_timing as timing, data::now, live_bars as bars, live_control::Control};
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
        data::{
            SubscribeBars, SubscribeCustomData, SubscribeQuotes, UnsubscribeBars,
            UnsubscribeCustomData, UnsubscribeQuotes,
        },
    },
};
use nautilus_model::{
    data::{BarType, CustomData, Data, QuoteTick},
    identifiers::{ClientId, Venue},
    instruments::{FuturesContract, InstrumentAny},
    types::Price,
};
use std::{any::Any, cell::RefCell, rc::Rc, sync::atomic::Ordering};
use tokio::task::JoinHandle;
/// Select against a single request-start cutoff, not a later response time.
/// No history mutation occurs until every expected completed candle validates.

#[derive(Debug, Default)]
struct TailStability {
    close_ns: u64,
    ohlc_bits: Option<[u64; 4]>,
    unchanged_since_ns: u64,
}

impl TailStability {
    fn observe(&mut self, candle: &Candle, close_ns: u64, requested_at: u64) -> bool {
        if requested_at < close_ns.saturating_add(timing::FINALIZATION_DELAY_NS) {
            return false;
        }
        let bits = [
            candle.open.to_bits(),
            candle.high.to_bits(),
            candle.low.to_bits(),
            candle.close.to_bits(),
        ];
        if self.close_ns != close_ns || self.ohlc_bits != Some(bits) {
            self.close_ns = close_ns;
            self.ohlc_bits = Some(bits);
            self.unchanged_since_ns = requested_at;
            return false;
        }
        requested_at.saturating_sub(self.unchanged_since_ns) >= timing::STABILITY_CONFIRM_NS
    }

    fn admitted(&mut self, close_ns: u64) {
        if self.close_ns <= close_ns {
            *self = Self::default();
        }
    }
}

fn validate_live_update(update: &super::history_revision::Update, was_paused: bool) -> Result<()> {
    ensure!(
        update.price_revised == 0,
        "Previously admitted broker candle changed OHLC; live trading requires review"
    );
    ensure!(
        !(was_paused && !update.new.is_empty()),
        "Data recovery crossed a completed strategy bar; catch-up trading is disabled"
    );
    ensure!(
        update.new.len() <= 1,
        "More than one completed strategy bar arrived at once; catch-up trading is disabled"
    );
    Ok(())
}

async fn wait_for_simulated_order(control: &Control) {
    loop {
        if control.stopping.load(Ordering::Acquire) {
            return;
        }
        let deadline = control.order_deadline.load(Ordering::Acquire);
        if deadline == 0 || now() >= deadline {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn prepare_update(
    history: &mut super::history_revision::History,
    stability: &mut TailStability,
    candles: Vec<Candle>,
    requested_at: u64,
    date: chrono::NaiveDate,
    interval: Interval,
) -> Result<Option<super::history_revision::Update>> {
    let completed = bars::completed_for(candles, requested_at, interval)?;
    let latest_candle = completed.last().expect("completed bars");
    let latest = bars::close_for(latest_candle, interval)?;
    if timing::publication_pending(latest, requested_at, interval.nanoseconds()) {
        return Ok(None);
    }
    let known = history.latest_close();
    let pending_new = completed
        .iter()
        .map(|c| bars::close_for(c, interval))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|close| *close > known)
        .count();
    ensure!(
        pending_new <= 1,
        "More than one completed strategy bar is pending; live catch-up is unsafe"
    );
    if latest > known && !stability.observe(latest_candle, latest, requested_at) {
        return Ok(None);
    }
    let update = history.update(completed, date, requested_at)?;
    if let Some(last) = update.new.last() {
        stability.admitted(bars::close_for(last, interval)?);
    }
    Ok(Some(update))
}

fn next_live_poll(
    now_ns: u64,
    latest_close: u64,
    step: u64,
    date: chrono::NaiveDate,
    calendar: &super::session_calendar::Calendar,
) -> Result<u64> {
    let (session_start, _) = calendar.bounds(date)?;
    let first_close = session_start.saturating_add(step);
    let first_finalized = first_close.saturating_add(timing::FINALIZATION_DELAY_NS);
    if latest_close < first_close && now_ns < first_finalized {
        return Ok(first_finalized);
    }
    Ok(timing::next_poll(now_ns, latest_close, step))
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
    pub emit_intrabar_ticks: bool,
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
        "SqueezeMomentumBars"
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
                let full_tick = |price: f64, ts: u64| -> Result<(Data, Data)> {
                    let snapshot =
                        super::synthetic::full_snapshot_for(c.token, price.round() as i32, ts, 1);
                    let quote = kite_adapter::mapping::quotes::map(&snapshot, &c.instrument)?
                        .ok_or_else(|| anyhow!("Synthetic full tick did not map to quote"))?;
                    let full = kite_adapter::data::full_tick::KiteFullTick { snapshot, quote };
                    Ok((
                        Data::Quote(quote),
                        Data::Custom(CustomData::from_arc(std::sync::Arc::new(full))),
                    ))
                };
                let emit_full = |price: f64, ts: u64| -> Result<()> {
                    let (quote, full) = full_tick(price, ts)?;
                    emit(quote)?;
                    emit(full)
                };
                for bar in &c.warmup {
                    emit(Data::Bar(bars::bar_for(bar, bt, now(), c.interval)?))?;
                }
                if c.control.sim {
                    for (index, bar) in c.simulated.iter().enumerate() {
                        if c.control.stopping.load(Ordering::Acquire) {
                            break;
                        }
                        let close = bars::close_for(bar, c.interval)?;
                        let open = close - c.interval.nanoseconds();
                        if c.emit_intrabar_ticks {
                            let previous = index
                                .checked_sub(1)
                                .and_then(|i| c.simulated.get(i))
                                .map_or(bar.open, |b| b.close);
                            let rising = bar.close >= previous;
                            let shock = if rising {
                                bar.high + 55.0
                            } else {
                                bar.low - 55.0
                            };
                            emit_full(bar.open, open + 1_000_000_000)?;
                            tokio::time::sleep(std::time::Duration::from_millis(
                                c.synthetic_delay_ms,
                            ))
                            .await;
                            emit_full(shock, open + 2_000_000_000)?;
                            tokio::time::sleep(std::time::Duration::from_millis(
                                c.synthetic_delay_ms,
                            ))
                            .await;
                            emit_full(shock, open + 4_000_000_000)?;
                            tokio::time::sleep(std::time::Duration::from_millis(
                                c.synthetic_delay_ms,
                            ))
                            .await;
                            emit_full(bar.close, close - 2_000_000_000)?;
                        } else {
                            emit(quote(bar.open, open + 2))?;
                            tokio::time::sleep(std::time::Duration::from_millis(
                                c.synthetic_delay_ms,
                            ))
                            .await;
                            emit(quote(bar.open, open + 3))?;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(c.synthetic_delay_ms))
                            .await;
                        if c.emit_intrabar_ticks {
                            wait_for_simulated_order(&c.control).await;
                        }
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
                let mut history = super::history_revision::History::new_for(
                    &c.warmup,
                    c.calendar.clone(),
                    c.interval,
                )?;
                let step = c.interval.nanoseconds();
                let mut stability = TailStability::default();
                let mut reader = historical::Reader::default();
                let mut next_fetch =
                    next_live_poll(now(), history.latest_close(), step, c.date, &c.calendar)?;
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
                        prepare_update(
                            &mut history,
                            &mut stability,
                            candles,
                            requested_at,
                            c.date,
                            c.interval,
                        )
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
                            next_fetch = next_live_poll(
                                received_at,
                                history.latest_close(),
                                step,
                                c.date,
                                &c.calendar,
                            )?;
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
                    next_fetch =
                        next_live_poll(now(), history.latest_close(), step, c.date, &c.calendar)?;
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
                    let was_paused = c.control.paused.load(Ordering::Acquire);
                    if let Err(error) = validate_live_update(&update, was_paused) {
                        {
                            let mut stats = c.control.bar_feed.lock().expect("bar feed stats");
                            stats.last_rebuild_reason = error.to_string();
                        }
                        c.control.fail(&format!("Bar finalization: {error}"));
                        return Err(error);
                    }
                    if was_paused {
                        c.control.paused.store(false, Ordering::Release);
                    }
                    for bar in update.new {
                        emit(Data::Bar(bars::bar_for(&bar, bt, received_at, c.interval)?))?;
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
    fn subscribe(&mut self, cmd: SubscribeCustomData) -> Result<()> {
        ensure!(
            self.config.control.sim
                && self.config.emit_intrabar_ticks
                && cmd.data_type.type_name() == "KiteFullTick",
            "Only realtime Ribbon simulation may subscribe to synthetic KiteFullTick data"
        );
        self.begin()
    }
    fn unsubscribe(&mut self, _: &UnsubscribeCustomData) -> Result<()> {
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
#[path = "live_data_tests.rs"]
mod polling_tests;

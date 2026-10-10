//! Native order dispatch shared by the mock, sandbox and production factories.
//!
//! kite-adapter 0.7.0, "place the order, reconcile, done":
//! * **Placing** an order makes no Kite read. Admission is local and in memory: stream
//!   up (entries only), nothing owned still unresolved, the strategy's position equal to
//!   the owned fills, the contract cap and the exposure rule, then the order-rate budget
//!   (Redis). Then one place call.
//! * **Reconciliation** (`refresh`) reads the order book and the trade book together and
//!   turns owned orders into Nautilus events. It runs on every order-stream update, every
//!   2 s while an owned order is unresolved, and every 15 s otherwise.
//! * **Account audit** (`audit`) reads positions every 15 s: a position in any other
//!   contract or product halts at once; Kite's position for this contract may trail the
//!   owned fills for up to `AUDIT_GRACE` before the run halts.
use super::super::{
    broker_events::{self, BrokerOrder, BrokerTrade, ObservationLag, Ownership},
    native,
    request::Command,
    transport::Outcome,
};
use super::{
    broker::{Broker, BrokerPosition},
    ledger::{Record, Store},
    outage::{self, ReadFailure},
};
use anyhow::{Result, anyhow, ensure};
use nautilus_common::{factories::OrderEventFactory, messages::ExecutionEvent};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{
    events::OrderEventAny,
    identifiers::ClientOrderId,
    orders::{Order, OrderAny},
};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedSender;
/// How long Kite's position for this contract may differ from the owned fills before the
/// run halts. Kite's positions trail its trade book by a moment; the audit runs every
/// 15 s, so a real mismatch is reported by the second or third audit.
pub(crate) const AUDIT_GRACE: Duration = Duration::from_secs(20);
/// Book reads per reconciliation while orders and trades disagree (`ObservationLag`),
/// and the pause between them. Still lagging after that: nothing changes, the next pass
/// (order update, 2 s pending timer) reads again.
const BOOK_READS: u32 = 3;
const BOOK_LAG_PAUSE: Duration = Duration::from_millis(150);
/// Order-path wall-clock timestamps (ns) for the latency log (kite-adapter 0.7.1).
#[derive(Clone, Copy, Debug, Default)]
struct Timing {
    /// The strategy created the order (`ts_init`).
    created: u64,
    /// Admission started (the dispatcher had the order).
    admitting: u64,
    /// The place call went out / came back.
    sent: u64,
    acked: u64,
}
fn wall_ns() -> u64 {
    nautilus_core::time::get_atomic_clock_realtime().get_time_ns().as_u64()
}
fn ms(later: u64, earlier: u64) -> f64 {
    (later as f64 - earlier as f64) / 1e6
}
/// One latency JSON line on stdout (the run log), never panicking.
fn latency(value: serde_json::Value) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}
struct PreparedObservation {
    position: i64,
    records: BTreeMap<String, Record>,
    events: Vec<OrderEventAny>,
}
pub(crate) struct Dispatcher {
    broker: Arc<dyn Broker>,
    store: Box<dyn Store>,
    factory: OrderEventFactory,
    product: String,
    token: u32,
    instrument_id: String,
    symbol: String,
    records: BTreeMap<String, Record>,
    poisoned: bool,
    /// Net position from the owned fills (BUY +, SELL −).
    position: i64,
    /// Per-order and per-position contract cap (broker settings `max_lots`, default 1).
    max_lots: i64,
    /// When the account audit first saw Kite's position differ from the owned fills.
    audit_mismatch: Option<Instant>,
    /// `AUDIT_GRACE`, shortened only by tests.
    audit_grace: Duration,
    /// Latency log: per client order id, until the order is closed.
    timings: BTreeMap<String, Timing>,
    /// What started the next reconciliation and when (ns): an order-stream update, the
    /// pending or fallback timer, a reconnect, the paper poll. Taken by `refresh`.
    trigger: Option<(u64, &'static str)>,
}
impl Dispatcher {
    #[cfg(test)]
    pub fn unresolved(&self) -> usize {
        self.records
            .values()
            .filter(|r| OrderAny::from_events(r.events.clone()).map_or(true, |o| !o.is_closed()))
            .count()
    }
    #[cfg(test)]
    pub fn with_audit_grace(mut self, grace: Duration) -> Self {
        self.audit_grace = grace;
        self
    }
    /// Stops all further dispatch for this run (in memory; the next run starts fresh).
    pub fn fault(&mut self) {
        self.poisoned = true;
    }
    /// Final reconciliation at shutdown: the book until nothing owned is unresolved
    /// (bounded), then Kite's position must equal the owned fills and be flat.
    pub async fn finish(
        &mut self,
        tx: &UnboundedSender<ExecutionEvent>,
        failed: bool,
    ) -> Result<()> {
        let mut settled = false;
        if !self.poisoned && !failed {
            settled = async {
                for pass in 0..5 {
                    self.refresh(tx).await?;
                    if !self.has_unresolved() {
                        break;
                    }
                    if pass < 4 {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
                if self.has_unresolved() {
                    return Ok(false);
                }
                // Positions can trail the last fill by a moment.
                for pass in 0..5 {
                    if self.audit_once().await? {
                        return Ok::<bool, anyhow::Error>(true);
                    }
                    if pass < 4 {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
                Ok(false)
            }
            .await
            .unwrap_or(false);
            if !settled {
                self.fault();
            }
        }
        let clean = settled
            && !failed
            && !self.poisoned
            && !self.has_unresolved()
            && self.position == 0;
        if !clean {
            self.poisoned = true;
        }
        ensure!(
            clean,
            "Stopped with unresolved orders, exposure or a failed reconciliation: check positions and open orders in Kite"
        );
        Ok(())
    }
    pub fn has_unresolved(&self) -> bool {
        self.records
            .values()
            .any(|r| OrderAny::from_events(r.events.clone()).map_or(true, |o| !o.is_closed()))
    }

    pub fn ownership(&self) -> BTreeMap<String, ClientOrderId> {
        self.records
            .iter()
            .filter_map(|(id, r)| {
                r.broker_id
                    .as_ref()
                    .map(|b| (b.clone(), ClientOrderId::from(id.as_str())))
            })
            .collect()
    }

    pub fn new(
        broker: Arc<dyn Broker>,
        store: Box<dyn Store>,
        factory: OrderEventFactory,
        product: String,
        token: u32,
        instrument_id: String,
        symbol: String,
    ) -> Self {
        Self {
            broker,
            store,
            factory,
            product,
            token,
            instrument_id,
            symbol,
            records: BTreeMap::new(),
            poisoned: false,
            position: 0,
            max_lots: 1,
            audit_mismatch: None,
            audit_grace: AUDIT_GRACE,
            timings: BTreeMap::new(),
            trigger: None,
        }
    }
    /// Records what is about to start the next reconciliation (latency log).
    pub fn note_trigger(&mut self, at_ns: u64, kind: &'static str) {
        self.trigger = Some((at_ns, kind));
    }
    /// Raises the contract cap from 1 (reviewed `max_lots` setting, 1..=10).
    pub fn with_max_lots(mut self, lots: u32) -> Self {
        self.max_lots = i64::from(lots.clamp(1, 10));
        self
    }
    fn now() -> UnixNanos {
        nautilus_core::time::get_atomic_clock_realtime().get_time_ns()
    }
    /// One JSON line on stderr. Unlike `eprintln!`, never panics when stderr is gone
    /// (e.g. the terminal closed): a log line must not kill the order path.
    fn warn(value: serde_json::Value) {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "{value}");
    }
    fn emit(tx: &UnboundedSender<ExecutionEvent>, event: OrderEventAny) -> Result<()> {
        tx.send(ExecutionEvent::Order(event))
            .map_err(|_| anyhow!("Native execution event channel closed"))
    }
    /// A rate-limited read starts the shared cooldown before the error is returned.
    fn cooldown_on(&mut self, error: &anyhow::Error) -> Result<()> {
        if let Some(ReadFailure::RateLimited(ms)) = error.downcast_ref::<ReadFailure>() {
            self.store.cooldown(*ms)?;
        }
        Ok(())
    }
    pub async fn submit(
        &mut self,
        order: OrderAny,
        position: i64,
        tx: &UnboundedSender<ExecutionEvent>,
    ) -> Result<()> {
        self.submit_guarded(order, position, tx, None).await
    }
    /// Local admission, in memory only: no Kite read.
    fn admit(
        &self,
        order: &OrderAny,
        position: i64,
        stream_ready: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<()> {
        use nautilus_model::enums::OrderSide;
        ensure!(
            order.is_reduce_only()
                || stream_ready
                    .is_none_or(|ready| ready.load(std::sync::atomic::Ordering::Acquire)),
            "Kite order stream recovering; new entries paused"
        );
        // One order at a time: a resting protective stop and an exit, or two entries,
        // could otherwise both fill (Kite does not enforce reduce-only).
        ensure!(
            !self.has_unresolved(),
            "Another native order remains unresolved"
        );
        // The strategy acts on the position the reconciled fills produced; anything else
        // means it has not seen the latest fill yet.
        ensure!(
            position == self.position,
            "Strategy and owned-fill positions differ"
        );
        let qty = order.quantity().as_decimal();
        let held = rust_decimal::Decimal::from(position.unsigned_abs());
        let opposing = matches!(
            (order.order_side(), position.signum()),
            (OrderSide::Buy, -1) | (OrderSide::Sell, 1)
        );
        let resulting = rust_decimal::Decimal::from(position)
            + if order.order_side() == OrderSide::Buy { qty } else { -qty };
        // The cap applies to the position before and after the order, so a flip
        // order may be larger than `max_lots` (e.g. +3 → −3 is one SELL 6).
        ensure!(
            qty >= rust_decimal::Decimal::ONE
                && qty.fract().is_zero()
                && position.abs() <= self.max_lots
                && resulting.abs() <= rust_decimal::Decimal::from(self.max_lots),
            "Native account contract cap exceeded"
        );
        // Allowed: a reduce-only exit, an entry from flat, or ONE flip order that closes
        // the whole open position and opens the opposite side (qty > held). Never adds,
        // and a non-reduce order on an open position must cross zero.
        ensure!(
            if order.is_reduce_only() {
                position != 0 && qty <= held && opposing
            } else {
                position == 0 || (opposing && qty > held)
            },
            "Exposure requires a reducing exit, an entry from flat, or a full flip"
        );
        Ok(())
    }
    pub async fn submit_guarded(
        &mut self,
        order: OrderAny,
        position: i64,
        tx: &UnboundedSender<ExecutionEvent>,
        stream_ready: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<()> {
        ensure!(!self.poisoned, "Native dispatcher stopped after a fault");
        let mut timing = Timing {
            created: order.ts_init().as_u64(),
            admitting: wall_ns(),
            ..Timing::default()
        };
        let id = order.client_order_id().to_string();
        ensure!(
            !self.records.contains_key(&id),
            "Native order was already attempted; no resubmission"
        );
        let tag = UUID4::new().to_string().replace('-', "")[..20].to_owned();
        let command = match self
            .admit(&order, position, stream_ready)
            .and_then(|()| native::submit_with_position(&order, &self.product, &tag, position))
        {
            Ok(command) => command,
            Err(e) => {
                return Self::emit(
                    tx,
                    self.factory.generate_order_denied(
                        &order,
                        &format!("Kite native admission: {e:#}"),
                        Self::now(),
                    ),
                );
            }
        };
        // Order-rate budget: a Redis error or an exhausted budget denies a new entry, but
        // never blocks an exit (Kite's own limits are higher than this budget).
        if let Err(e) = self.store.reserve() {
            if order.is_reduce_only() {
                Self::warn(serde_json::json!({"event":"native_budget_bypassed",
                    "client_order_id":id,"reduce_only":true,"reason":format!("{e:#}")}));
            } else {
                return Self::emit(
                    tx,
                    self.factory.generate_order_denied(
                        &order,
                        &format!("Order-rate budget: {e:#}"),
                        Self::now(),
                    ),
                );
            }
        }
        let submitted = self.factory.generate_order_submitted(&order, Self::now());
        let mut record = Record {
            events: order.events().iter().map(|e| (*e).clone()).collect(),
            tag,
            product: self.product.clone(),
            token: self.token,
            broker_id: None,
            outcome: "Dispatching".into(),
            management: BTreeMap::new(),
        };
        record.events.push(submitted.clone());
        self.poisoned = true;
        self.store.save(&id, &record)?;
        self.records.insert(id.clone(), record.clone());
        Self::emit(tx, submitted)?;
        timing.sent = wall_ns();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(6),
            self.broker.execute(&command),
        )
        .await
        .unwrap_or(Ok(Outcome::Unknown));
        timing.acked = wall_ns();
        match response {
            Ok(Outcome::Acknowledged { order_id }) => {
                record.broker_id = Some(order_id);
                record.outcome = "Acknowledged".into();
            }
            Ok(Outcome::Rejected) => {
                record.outcome = "Rejected".into();
                record.events.push(self.factory.generate_order_rejected(
                    &order,
                    "Kite rejected submission",
                    Self::now(),
                    Self::now(),
                    false,
                ));
            }
            Ok(Outcome::RateLimited { retry_after_ms }) => {
                self.store.cooldown(retry_after_ms)?;
                record.outcome = "RateLimited".into();
            }
            Ok(Outcome::SessionExpired) => {
                record.outcome = "SessionExpired".into();
            }
            _ => {
                record.outcome = "Unknown".into();
            }
        }
        self.store.save(&id, &record)?;
        latency(serde_json::json!({"event":"latency_order_sent","order":id,
            "instrument":self.instrument_id,"outcome":record.outcome,
            "created_ms":timing.created / 1_000_000,
            "strategy_to_send_ms":ms(timing.sent, timing.created),
            "admission_ms":ms(timing.sent, timing.admitting),
            "place_call_ms":ms(timing.acked, timing.sent)}));
        if record.outcome == "Acknowledged" || record.outcome == "Unknown" {
            self.timings.insert(id.clone(), timing);
        }
        self.records.insert(id, record.clone());
        self.poisoned = false;
        if record.outcome == "Rejected" {
            Self::emit(tx, record.events.last().expect("rejection").clone())?;
        }
        if matches!(record.outcome.as_str(), "SessionExpired" | "RateLimited") {
            self.fault();
            anyhow::bail!("Kite authentication or rate limit stopped dispatch; check positions and open orders in Kite");
        }
        // Acknowledgement alone emits neither Accepted nor Filled.
        Ok(())
    }
    /// Reconciliation: the order book and the trade book (one round trip) turned into
    /// events for every owned order.
    pub async fn refresh(&mut self, tx: &UnboundedSender<ExecutionEvent>) -> Result<()> {
        ensure!(!self.poisoned, "Native dispatcher stopped after a fault");
        let started = wall_ns();
        let (trigger_at, trigger) = self.trigger.take().unwrap_or((started, "direct"));
        let broker = self.broker.clone();
        let mut prepared = None;
        let mut reads = 0_u32;
        for read in 1..=BOOK_READS {
            reads = read;
            let (orders, trades) = match outage::read(|| broker.book()).await {
                Ok(book) => book,
                Err(e) => {
                    self.cooldown_on(&e)?;
                    return Err(e);
                }
            };
            match self.prepare_observation(&orders, &trades) {
                Ok(p) => {
                    prepared = Some(p);
                    break;
                }
                Err(e) if e.is::<ObservationLag>() => {
                    if read < BOOK_READS {
                        tokio::time::sleep(BOOK_LAG_PAUSE).await;
                    } else {
                        Self::warn(serde_json::json!({"event":"native_book_lagging",
                            "reads":BOOK_READS,"reason":format!("{e}")}));
                    }
                }
                Err(e) => return Err(e),
            }
        }
        let Some(prepared) = prepared else {
            // Orders and trades still disagree: nothing is applied; the next pass reads
            // again (order update, 2 s pending timer, shutdown).
            return Ok(());
        };
        // Every order and trade is validated before any state changes. A failed record
        // update or event delivery poisons this dispatcher.
        self.poisoned = true;
        self.position = prepared.position;
        for (id, record) in &prepared.records {
            self.store.save(id, record)?;
        }
        self.records.extend(prepared.records);
        let read_done = wall_ns();
        let fills: Vec<(String, f64, u64)> = prepared
            .events
            .iter()
            .filter_map(|e| match e {
                OrderEventAny::Filled(f) => Some((
                    f.client_order_id.to_string(),
                    f.last_qty.as_f64(),
                    f.ts_event.as_u64(),
                )),
                _ => None,
            })
            .collect();
        for event in prepared.events {
            Self::emit(tx, event)?;
        }
        self.poisoned = false;
        let applied = wall_ns();
        for (id, qty, kite_fill) in fills {
            let Some(t) = self.timings.get(&id).copied() else { continue };
            // Kite's fill time has 1 s resolution: `after_kite_fill_ms` is approximate.
            latency(serde_json::json!({"event":"latency_fill","order":id,
                "instrument":self.instrument_id,"qty":qty,"trigger":trigger,
                "trigger_after_ack_ms":ms(trigger_at, t.acked),
                "reads":reads,"read_ms":ms(read_done, started),
                "applied_after_trigger_ms":ms(applied, trigger_at),
                "send_to_applied_ms":ms(applied, t.sent),
                "strategy_to_applied_ms":ms(applied, t.created),
                "kite_fill_s":kite_fill / 1_000_000_000,
                "applied_after_kite_fill_ms":ms(applied, kite_fill)}));
        }
        let records = &self.records;
        self.timings.retain(|id, _| {
            records
                .get(id)
                .is_some_and(|r| OrderAny::from_events(r.events.clone()).is_ok_and(|o| !o.is_closed()))
        });
        Ok(())
    }
    /// Background account audit (every 15 s): see `audit_once`. A mismatch of this
    /// contract's position is tolerated for `audit_grace`, then the run halts.
    pub async fn audit(&mut self) -> Result<()> {
        ensure!(!self.poisoned, "Native dispatcher stopped after a fault");
        if self.audit_once().await? {
            if let Some(since) = self.audit_mismatch.take() {
                Self::warn(serde_json::json!({"event":"native_position_converged",
                    "after_ms":since.elapsed().as_millis() as u64}));
            }
            return Ok(());
        }
        let since = *self.audit_mismatch.get_or_insert_with(Instant::now);
        ensure!(
            since.elapsed() < self.audit_grace,
            "Kite position differs from the owned fills for {} s; check positions and open orders in Kite",
            since.elapsed().as_secs()
        );
        Ok(())
    }
    /// One positions read. `Err`: a position in any other contract or product (never
    /// lag; halts at once). `Ok(false)`: Kite's position for this contract differs from
    /// the owned fills (may be lag). `Ok(true)`: they match.
    async fn audit_once(&mut self) -> Result<bool> {
        let broker = self.broker.clone();
        let positions = match outage::read(|| broker.positions()).await {
            Ok(p) => p,
            Err(e) => {
                self.cooldown_on(&e)?;
                return Err(e);
            }
        };
        let ours = |p: &BrokerPosition| {
            p.exchange == "MCX"
                && p.tradingsymbol == self.symbol
                && p.product == self.product
                && p.instrument_token == self.token
        };
        ensure!(
            positions.iter().filter(|p| p.quantity != 0).all(ours),
            "Unmanaged account exposure"
        );
        let held: i64 = positions.iter().filter(|p| ours(p)).map(|p| p.quantity).sum();
        if held != self.position {
            Self::warn(serde_json::json!({"event":"native_position_mismatch",
                "instrument":self.instrument_id,"kite":held,"owned_fills":self.position}));
        }
        Ok(held == self.position)
    }
    fn prepare_observation(
        &self,
        orders: &[BrokerOrder],
        trades: &[BrokerTrade],
    ) -> Result<PreparedObservation> {
        // An open order nobody here placed (manual, another program) halts at once.
        ensure!(
            orders
                .iter()
                .filter(|o| !matches!(o.status.as_str(), "COMPLETE" | "CANCELLED" | "REJECTED"))
                .all(|o| self
                    .records
                    .values()
                    .any(|r| o.tag.as_deref() == Some(r.tag.as_str()))),
            "Unowned open account order"
        );
        let mut prepared = PreparedObservation {
            position: self.position,
            records: BTreeMap::new(),
            events: Vec::new(),
        };
        for (id, record) in &self.records {
            let current = OrderAny::from_events(record.events.clone())?;
            let matches: Vec<_> = orders
                .iter()
                .filter(|b| b.tag.as_deref() == Some(record.tag.as_str()))
                .collect();
            if let Some(existing) = &record.broker_id {
                ensure!(
                    orders
                        .iter()
                        .filter(|b| &b.order_id == existing)
                        .all(|b| b.tag.as_deref() == Some(record.tag.as_str())),
                    "Broker order ownership changed"
                );
            }
            if matches.is_empty() {
                // OMS acknowledgement can precede visibility in the daily book.
                // Retain the unresolved record and poll reads; never resubmit.
                continue;
            }
            ensure!(matches.len() == 1, "Broker tag is ambiguous");
            let broker = matches[0];
            if let Some(existing) = &record.broker_id {
                ensure!(
                    existing == &broker.order_id,
                    "Broker order ownership changed"
                );
            }
            let owner = Ownership {
                broker_id: &broker.order_id,
                tag: &record.tag,
                product: &record.product,
                token: record.token,
            };
            let mut normalized = broker.clone();
            let mut stop_modified = None::<i64>;
            if !record.management.is_empty() {
                use rust_decimal::prelude::ToPrimitive;
                ensure!(
                    record.management.len() == 1,
                    "Ambiguous outstanding broker management commands"
                );
                let status = record.management.values().next().expect("management");
                if let Some(rest) = status.strip_prefix("StopModify:") {
                    let (value, phase) = rest
                        .split_once(':')
                        .ok_or_else(|| anyhow!("Malformed persisted stop modification"))?;
                    let requested: i64 = value.parse()?;
                    let previous = current
                        .trigger_price()
                        .ok_or_else(|| anyhow!("Stop trigger missing"))?
                        .as_decimal()
                        .to_i64()
                        .ok_or_else(|| anyhow!("Invalid stop trigger"))?;
                    let actual = broker
                        .trigger_price
                        .ok_or_else(|| anyhow!("Broker stop trigger missing"))?
                        .to_i64()
                        .ok_or_else(|| anyhow!("Invalid broker stop trigger"))?;
                    ensure!(
                        actual == previous || actual == requested,
                        "Unowned protective stop modification observed"
                    );
                    if actual == requested && actual != previous {
                        ensure!(
                            phase == "Acknowledged" && broker.status == "TRIGGER PENDING",
                            "Unconfirmed protective stop modification"
                        );
                        normalized.trigger_price = Some(rust_decimal::Decimal::from(previous));
                        // Kite's SL form carries a limit tied to the trigger; shift it
                        // with the trigger so the conversion band is judged consistently.
                        if normalized.order_type == "SL" {
                            normalized.price -= rust_decimal::Decimal::from(actual - previous);
                        }
                        stop_modified = Some(requested);
                    }
                }
            }
            let mut events = broker_events::reconcile(
                &current,
                &owner,
                &normalized,
                trades,
                &self.factory,
                Self::now(),
            )?;
            if let Some(trigger) = stop_modified {
                use nautilus_model::{
                    identifiers::VenueOrderId,
                    types::{Price, Quantity},
                };
                let latest = broker_events::timestamp(
                    broker
                        .exchange_update_timestamp
                        .as_deref()
                        .unwrap_or(&broker.order_timestamp),
                )?;
                let updated = self.factory.generate_order_updated(
                    &current,
                    VenueOrderId::from(broker.order_id.as_str()),
                    Quantity::from(broker.quantity),
                    None,
                    Some(Price::new(trigger as f64, 0)),
                    None,
                    latest,
                    Self::now(),
                );
                events.push(updated);
            }
            if record.broker_id.as_deref() != Some(broker.order_id.as_str()) || !events.is_empty() {
                let mut next = record.clone();
                next.broker_id = Some(broker.order_id.clone());
                next.outcome = "Observed".into();
                next.events.extend(events.iter().cloned());
                if stop_modified.is_some() {
                    next.management.clear();
                }
                prepared.records.insert(id.clone(), next);
                prepared.events.extend(events);
            }
        }

        let mut owned = rust_decimal::Decimal::ZERO;
        for (id, record) in &self.records {
            let record = prepared.records.get(id).unwrap_or(record);
            let order = OrderAny::from_events(record.events.clone())?;
            owned += if order.order_side() == nautilus_model::enums::OrderSide::Buy {
                order.filled_qty().as_decimal()
            } else {
                -order.filled_qty().as_decimal()
            };
        }
        use rust_decimal::prelude::ToPrimitive;
        prepared.position = owned
            .to_i64()
            .ok_or_else(|| anyhow!("Invalid owned position"))?;
        ensure!(
            prepared.position.abs() <= self.max_lots,
            "Owned position exceeds contract cap"
        );
        Ok(prepared)
    }
    pub async fn modify_stop(
        &mut self,
        id: ClientOrderId,
        command_id: UUID4,
        new_trigger: i64,
    ) -> Result<()> {
        use nautilus_model::enums::{OrderSide, OrderStatus, OrderType};
        use rust_decimal::prelude::ToPrimitive;
        ensure!(!self.poisoned, "Native dispatcher stopped after a fault");
        ensure!(new_trigger > 0, "Invalid protective stop trigger");
        // Protective: never blocked by the order-rate budget (see `submit_guarded`).
        if let Err(e) = self.store.reserve() {
            Self::warn(serde_json::json!({"event":"native_budget_bypassed",
                "client_order_id":id.to_string(),"command":"modify_stop","reason":format!("{e:#}")}));
        }
        let record = self
            .records
            .get(id.as_str())
            .ok_or_else(|| anyhow!("Unowned protective stop"))?
            .clone();
        ensure!(
            record.management.is_empty(),
            "Protective stop modification already unresolved"
        );
        let current = OrderAny::from_events(record.events.clone())?;
        ensure!(
            current.order_type() == OrderType::StopMarket
                && current.is_reduce_only()
                && current.status() == OrderStatus::Accepted,
            "Only accepted reduce-only SL-M orders can be modified"
        );
        let old = current
            .trigger_price()
            .ok_or_else(|| anyhow!("Protective trigger missing"))?
            .as_decimal()
            .to_i64()
            .ok_or_else(|| anyhow!("Invalid protective trigger"))?;
        ensure!(
            if current.order_side() == OrderSide::Sell {
                new_trigger >= old && self.position == 1
            } else {
                new_trigger <= old && self.position == -1
            },
            "Stop update cannot loosen risk or mismatch exposure"
        );
        let broker_id = record
            .broker_id
            .clone()
            .ok_or_else(|| anyhow!("Protective stop broker identity unknown"))?;
        // No Kite read here (kite-adapter 0.7.0): the record is `Accepted` at `old` only
        // because a reconciliation saw the stop resting at Kite with that trigger, and
        // Kite itself refuses to modify an order that has since filled or been cancelled
        // (an uncertain answer stops the run, below).
        let command = Command::ModifyProtectiveStop {
            order_id: broker_id.clone(),
            quantity: 1,
            trigger_price_rupees: new_trigger,
            market_protection: -1,
        };
        command.validate()?;
        let key = command_id.to_string();
        let record = self.records.get_mut(id.as_str()).expect("record");
        record
            .management
            .insert(key.clone(), format!("StopModify:{new_trigger}:Dispatching"));
        self.poisoned = true;
        self.store.save(id.as_str(), record)?;
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(6),
            self.broker.execute(&command),
        )
        .await
        .unwrap_or(Ok(Outcome::Unknown));
        let accepted =
            matches!(&response,Ok(Outcome::Acknowledged{order_id}) if order_id==&broker_id);
        record.management.insert(
            key,
            format!(
                "StopModify:{new_trigger}:{}",
                if accepted { "Acknowledged" } else { "Unknown" }
            ),
        );
        self.store.save(id.as_str(), record)?;
        self.poisoned = false;
        if !accepted {
            self.fault();
            anyhow::bail!("Protective stop modification uncertain; manual review required");
        }
        Ok(())
    }
    pub async fn cancel(
        &mut self,
        id: ClientOrderId,
        command_id: UUID4,
        tx: &UnboundedSender<ExecutionEvent>,
    ) -> Result<()> {
        ensure!(!self.poisoned, "Native dispatcher stopped after a fault");
        // A cancel precedes an exit (SATS cancels its SL-M first): never blocked by the
        // order-rate budget (see `submit_guarded`).
        if let Err(e) = self.store.reserve() {
            Self::warn(serde_json::json!({"event":"native_budget_bypassed",
                "client_order_id":id.to_string(),"command":"cancel","reason":format!("{e:#}")}));
        }
        let record = self
            .records
            .get_mut(id.as_str())
            .ok_or_else(|| anyhow!("Cannot cancel an unowned native order"))?;
        let current = OrderAny::from_events(record.events.clone())?;
        ensure!(
            !current.is_closed() && record.management.is_empty(),
            "Order closed or cancellation already attempted"
        );
        let broker = record
            .broker_id
            .clone()
            .ok_or_else(|| anyhow!("Broker ownership unresolved"))?;
        let command = Command::Cancel {
            order_id: broker.clone(),
        };
        command.validate()?;
        let key = command_id.to_string();
        record.management.insert(key.clone(), "Dispatching".into());
        self.poisoned = true;
        self.store.save(id.as_str(), record)?;
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(6),
            self.broker.execute(&command),
        )
        .await
        .unwrap_or(Ok(Outcome::Unknown));
        let rejected = matches!(response, Ok(Outcome::Rejected));
        let expired = matches!(
            response,
            Ok(Outcome::SessionExpired) | Ok(Outcome::RateLimited { .. })
        );
        if let Ok(Outcome::RateLimited { retry_after_ms }) = &response {
            self.store.cooldown(*retry_after_ms)?;
        }
        record.management.insert(
            key,
            match response {
                Ok(Outcome::Acknowledged { order_id }) if order_id == broker => "Acknowledged",
                Ok(Outcome::Rejected) => "Rejected",
                _ => "Unknown",
            }
            .into(),
        );
        self.store.save(id.as_str(), record)?;
        self.poisoned = false;
        if expired {
            self.fault();
            anyhow::bail!("Kite authentication or rate limit stopped dispatch; review required");
        }
        if rejected {
            Self::emit(
                tx,
                self.factory.generate_order_cancel_rejected(
                    &current,
                    Some(broker.as_str().into()),
                    "Kite rejected cancellation",
                    Self::now(),
                    Self::now(),
                ),
            )?;
        }
        Ok(())
    }
}

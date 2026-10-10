//! Serial native command dispatch shared by explicit mock, sandbox and production factories.
use super::super::{
    broker_events::{self, Ownership},
    native,
    request::Command,
    transport::Outcome,
};
use super::{
    broker::Broker,
    ledger::{Record, Store},
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
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedSender;
/// Longest a clean reconciliation may back an order without a fresh REST read. The order
/// stream's fallback refresh runs every 15 s, so a healthy session always stays inside it.
pub(crate) const CACHED_ADMISSION_MAX_AGE: Duration = Duration::from_secs(20);
/// How long the full account snapshot may lag a postback fill (its trades missing from
/// `/trades`, its quantity missing from positions) before the run stops for review.
pub(crate) const POSTBACK_VERIFY_GRACE: Duration = Duration::from_secs(30);
struct PreparedObservation {
    position: i64,
    records: BTreeMap<String, Record>,
    events: Vec<OrderEventAny>,
    /// A postback fill is not yet visible in this snapshot (inside its grace period).
    pending: bool,
    /// Postback fills this snapshot confirmed trade for trade.
    verified: Vec<String>,
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
    position: i64,
    /// Per-order and per-position contract cap (broker settings `max_lots`, default 1).
    max_lots: i64,
    /// Bumped by the order stream on every account order update and connection change.
    doorbell: Arc<AtomicU64>,
    /// Opt-in (production): admit orders from the last clean reconciliation, no REST.
    cached_admission: bool,
    /// When the last clean reconciliation started and the doorbell value it covered.
    clean: Option<(Instant, u64)>,
    /// Orders admitted from cached state / from a full REST preflight.
    admitted_cached: u64,
    admitted_full: u64,
    /// Postback fills awaiting confirmation by the full snapshot:
    /// client order id → (Kite trade ids emitted, verification deadline).
    verifying: BTreeMap<String, (Vec<String>, Instant)>,
    /// Fills emitted from the postback fast path so far.
    postback_fills: u64,
    /// `POSTBACK_VERIFY_GRACE`, shortened only by tests.
    verify_grace: Duration,
    /// Postback fast path (2.20.1). OFF since 2.22.0: a fill it emits can be followed by a
    /// snapshot whose positions still lag, which the checks below treat as fatal, and a
    /// MARKET order's own post-submit refresh usually sees the fill first anyway. Kept,
    /// with its tests, until it is reworked; nothing in production turns it on.
    postback_fast_fill: bool,
}
impl Dispatcher {
    /// Enables the postback fast path (tests only until it is reworked).
    #[cfg(test)]
    pub fn set_postback_fast_fill(&mut self, enabled: bool) {
        self.postback_fast_fill = enabled;
    }
    /// Shared counter the order stream bumps; any change invalidates cached admission.
    pub fn doorbell(&self) -> Arc<AtomicU64> {
        self.doorbell.clone()
    }
    /// Admit orders from the last clean reconciliation instead of a REST preflight.
    pub fn with_cached_admission(mut self, enabled: bool) -> Self {
        self.cached_admission = enabled;
        self
    }
    /// Record a clean account observation started at `at`, covering doorbell value `bell`.
    pub fn mark_clean(&mut self, at: Instant, bell: u64) {
        if !self.poisoned {
            self.clean = Some((at, bell));
        }
    }
    /// (cached, full) preflight admissions so far.
    pub fn admissions(&self) -> (u64, u64) {
        (self.admitted_cached, self.admitted_full)
    }
    /// True while the last clean reconciliation still describes the account: recent, and
    /// no order update or connection change has arrived since it started.
    fn cached_fresh(&self) -> bool {
        self.cached_admission
            && !self.poisoned
            && self.clean.is_some_and(|(at, bell)| {
                at.elapsed() < CACHED_ADMISSION_MAX_AGE
                    && self.doorbell.load(Ordering::Acquire) == bell
            })
    }
    #[cfg(test)]
    pub fn unresolved(&self) -> usize {
        self.records
            .values()
            .filter(|r| OrderAny::from_events(r.events.clone()).map_or(true, |o| !o.is_closed()))
            .count()
    }
    /// Stops all further dispatch for this run (in memory; the next run starts fresh).
    pub fn fault(&mut self) {
        self.poisoned = true;
        self.clean = None;
    }
    pub async fn finish(
        &mut self,
        tx: &UnboundedSender<ExecutionEvent>,
        failed: bool,
    ) -> Result<()> {
        if !self.poisoned && !failed && self.refresh(tx).await.is_err() {
            self.fault();
        }
        let clean = !failed
            && !self.poisoned
            && !self.has_unresolved()
            && self.verifying.is_empty()
            && self.position == 0;
        if self.cached_admission {
            let (cached, full) = self.admissions();
            eprintln!(
                "{}",
                serde_json::json!({"event":"native_admissions","cached":cached,"full_preflight":full,
                    "postback_fills":self.postback_fills})
            );
        }
        if !clean {
            self.poisoned = true;
        }
        ensure!(
            clean,
            "Stopped with unresolved orders, exposure or a failed reconciliation: check positions and open orders in Kite"
        );
        Ok(())
    }
    async fn snapshot(&mut self) -> Result<super::broker::Snapshot> {
        match super::outage::snapshot(self.broker.as_ref()).await {
            Ok(s) => Ok(s),
            Err(e) => {
                if let Some(super::outage::ReadFailure::RateLimited(ms)) =
                    e.downcast_ref::<super::outage::ReadFailure>()
                {
                    self.store.cooldown(*ms)?;
                }
                Err(e)
            }
        }
    }
    pub fn has_unresolved(&self) -> bool {
        self.records
            .values()
            .any(|r| OrderAny::from_events(r.events.clone()).map_or(true, |o| !o.is_closed()))
    }
    /// True while an owned order is unresolved or a postback fill awaits confirmation:
    /// the order stream's short pending timer must keep running full snapshots.
    pub fn needs_refresh(&self) -> bool {
        self.has_unresolved() || !self.verifying.is_empty()
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
            doorbell: Arc::new(AtomicU64::new(0)),
            cached_admission: false,
            clean: None,
            admitted_cached: 0,
            admitted_full: 0,
            verifying: BTreeMap::new(),
            postback_fills: 0,
            verify_grace: POSTBACK_VERIFY_GRACE,
            postback_fast_fill: false,
        }
    }
    /// Raises the contract cap from 1 (reviewed `max_lots` setting, 1..=10).
    pub fn with_max_lots(mut self, lots: u32) -> Self {
        self.max_lots = i64::from(lots.clamp(1, 10));
        self
    }
    #[cfg(test)]
    pub fn with_verify_grace(mut self, grace: Duration) -> Self {
        self.verify_grace = grace;
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
    pub async fn submit(
        &mut self,
        order: OrderAny,
        position: i64,
        tx: &UnboundedSender<ExecutionEvent>,
    ) -> Result<()> {
        self.submit_guarded(order, position, tx, None).await
    }
    pub async fn submit_guarded(
        &mut self,
        order: OrderAny,
        position: i64,
        tx: &UnboundedSender<ExecutionEvent>,
        stream_ready: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<()> {
        ensure!(!self.poisoned, "Native dispatcher requires manual recovery");
        let id = order.client_order_id().to_string();
        ensure!(
            !self.records.contains_key(&id),
            "Native order was already attempted; no resubmission"
        );
        let tag = UUID4::new().to_string().replace('-', "")[..20].to_owned();
        let admission = || -> Result<()> {
            ensure!(
                order.is_reduce_only()
                    || stream_ready
                        .is_none_or(|ready| ready.load(std::sync::atomic::Ordering::Acquire)),
                "Kite order stream recovering; new entries paused"
            );
            Ok(())
        };
        let preflight = async {
            admission()?;
            ensure!(
                self.records
                    .values()
                    .all(|r| OrderAny::from_events(r.events.clone()).is_ok_and(|o| o.is_closed())),
                "Another native order remains unresolved"
            );
            // Cached admission: the last clean reconciliation already proved no unmanaged
            // exposure, no unowned open order and the broker position. It still holds while
            // it is recent, no order update/reconnect arrived since, nothing owned is
            // unresolved and the native position matches it. Otherwise read REST as before.
            let cached =
                stream_ready.is_some() && self.cached_fresh() && self.position == position;
            if !cached {
                let snapshot = self.snapshot().await?;
                ensure!(
                    snapshot
                        .positions
                        .iter()
                        .filter(|p| p.quantity != 0)
                        .all(|p| p.exchange == "MCX"
                            && p.tradingsymbol == self.symbol
                            && p.product == self.product
                            && p.instrument_token == self.token),
                    "Unmanaged account exposure"
                );
                let reports = super::reports::positions_for(
                    &snapshot,
                    self.factory.account_id(),
                    &self.product,
                    self.token,
                    &self.instrument_id,
                    &self.symbol,
                    Self::now(),
                )?;
                let actual = reports[0].quantity.as_decimal();
                let signed =
                    if reports[0].position_side == nautilus_model::enums::PositionSide::Short {
                        -actual
                    } else {
                        actual
                    };
                ensure!(
                    signed == rust_decimal::Decimal::from(position),
                    "Broker and native position differ"
                );
                ensure!(
                    snapshot.orders.iter().all(|b| matches!(
                        b.status.as_str(),
                        "COMPLETE" | "CANCELLED" | "REJECTED"
                    )),
                    "Broker has an unresolved order for this contract"
                );
            }
            use nautilus_model::enums::OrderSide;
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
                    position != 0
                } else {
                    position == 0 || (opposing && qty > held)
                },
                "Exposure requires a reducing exit, an entry from flat, or a full flip"
            );
            // Recheck after awaited broker reads: the stream may have disconnected.
            admission()?;
            ensure!(
                !cached || self.cached_fresh(),
                "Account changed during cached admission"
            );
            native::submit_with_position(&order, &self.product, &tag, position)
                .map(|command| (command, cached))
        }
        .await;
        let command = match preflight {
            Ok((c, cached)) => {
                if cached {
                    self.admitted_cached += 1;
                } else {
                    self.admitted_full += 1;
                }
                c
            }
            Err(e) => {
                if e.downcast_ref::<super::outage::ReadFailure>().is_some() {
                    self.fault();
                    return Err(e);
                }
                return Self::emit(
                    tx,
                    self.factory.generate_order_denied(
                        &order,
                        "Kite native preflight rejected order",
                        Self::now(),
                    ),
                );
            }
        };
        // Order-rate budget: a Redis error or an exhausted budget denies a new entry, but
        // never blocks an exit (Kite's own limits are higher than this budget). Until 0.5.0
        // either case stopped the whole run, open position included.
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
        // The account state changes from here; the next order needs a new reconciliation.
        self.clean = None;
        Self::emit(tx, submitted)?;
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(6),
            self.broker.execute(&command),
        )
        .await
        .unwrap_or(Ok(Outcome::Unknown));
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
        self.records.insert(id, record.clone());
        self.poisoned = false;
        if record.outcome == "Rejected" {
            Self::emit(tx, record.events.last().expect("rejection").clone())?;
        }
        if matches!(record.outcome.as_str(), "SessionExpired" | "RateLimited") {
            self.fault();
            anyhow::bail!("Kite authentication or rate limit stopped dispatch; review required");
        }
        // Acknowledgement alone emits neither Accepted nor Filled.
        Ok(())
    }
    pub async fn refresh(&mut self, tx: &UnboundedSender<ExecutionEvent>) -> Result<()> {
        ensure!(!self.poisoned, "Native dispatcher requires manual recovery");
        // Read the doorbell before any REST read: an update arriving during the reads
        // leaves this observation unable to back cached admission.
        let observed = (Instant::now(), self.doorbell.load(Ordering::Acquire));
        self.clean = None;
        let broker = self.broker.clone();
        let result = super::outage::snapshot_checked(broker.as_ref(), |snapshot| {
            // Retain the exclusive dispatcher borrow across retries; Store is Send,
            // not Sync. Preparing an observation itself makes no state changes.
            let dispatcher = &mut *self;
            dispatcher.prepare_observation(snapshot)
        })
        .await;
        let (_, prepared) = match result {
            Ok(value) => value,
            Err(e) => {
                if let Some(super::outage::ReadFailure::RateLimited(ms)) =
                    e.downcast_ref::<super::outage::ReadFailure>()
                {
                    self.store.cooldown(*ms)?;
                }
                return Err(e);
            }
        };
        // Every order, trade and position is validated before any state changes. A failed
        // event delivery poisons this dispatcher.
        self.poisoned = true;
        self.position = prepared.position;
        for (id, record) in &prepared.records {
            self.store.save(id, record)?;
        }
        self.records.extend(prepared.records);
        for event in prepared.events {
            Self::emit(tx, event)?;
        }
        for id in &prepared.verified {
            if let Some((trades, deadline)) = self.verifying.remove(id) {
                let left = deadline.saturating_duration_since(Instant::now());
                eprintln!(
                    "{}",
                    serde_json::json!({"event":"native_postback_fill_verified","order":id,
                        "trades":trades.len(),
                        "after_ms":self.verify_grace.saturating_sub(left).as_millis() as u64})
                );
            }
        }
        self.poisoned = false;
        // A snapshot still missing a postback fill never backs cached admission.
        self.clean = (!prepared.pending).then_some(observed);
        Ok(())
    }
    /// Postback fast path (production order stream). On a COMPLETE postback for an owned,
    /// still-open order, read only that order and its trades (two parallel REST calls)
    /// and emit its fills with their real Kite trade IDs through the same `reconcile`
    /// checks as the full snapshot. The order record is updated before any event is
    /// emitted. The full snapshot that follows must then show the same trades, with
    /// unchanged quantity, price and time, within `POSTBACK_VERIFY_GRACE`; otherwise the
    /// run stops for review.
    ///
    /// `Ok(false)`: not taken (not owned, already closed, pending stop modification,
    /// not COMPLETE yet, read failed, trades lagging, any check failed). Nothing changed;
    /// the full reconciliation handles the update exactly as before. `Err` only when a
    /// record update or event delivery failed after the dispatcher began changing state.
    pub async fn fast_fill(
        &mut self,
        broker_id: &str,
        tx: &UnboundedSender<ExecutionEvent>,
    ) -> Result<bool> {
        use nautilus_model::enums::{OrderSide, OrderStatus};
        use rust_decimal::prelude::ToPrimitive;
        if !self.postback_fast_fill || self.poisoned {
            return Ok(false);
        }
        let Some((id, record)) = self
            .records
            .iter()
            .find(|(_, r)| r.broker_id.as_deref() == Some(broker_id))
            .map(|(id, r)| (id.clone(), r.clone()))
        else {
            return Ok(false);
        };
        if !record.management.is_empty() {
            return Ok(false);
        }
        let Ok(current) = OrderAny::from_events(record.events.clone()) else {
            return Ok(false);
        };
        if current.is_closed() {
            return Ok(false);
        }
        let started = Instant::now();
        let skip = |reason: String| -> Result<bool> {
            eprintln!(
                "{}",
                serde_json::json!({"event":"native_postback_fill_skipped","order":id,
                    "reason":reason,"ms":started.elapsed().as_millis() as u64})
            );
            Ok(false)
        };
        let (broker, trades) = match tokio::time::timeout(
            Duration::from_secs(3),
            self.broker.order_detail(broker_id),
        )
        .await
        {
            Ok(Ok(Some(detail))) => detail,
            Ok(Ok(None)) => return Ok(false),
            Ok(Err(e)) => return skip(format!("{e}")),
            Err(_) => return skip("order read timed out".into()),
        };
        if broker.status != "COMPLETE" {
            return skip(format!("status {}", broker.status));
        }
        if broker.tag.as_deref() != Some(record.tag.as_str()) {
            return skip("tag mismatch".into());
        }
        let owner = Ownership {
            broker_id,
            tag: &record.tag,
            product: &record.product,
            token: record.token,
        };
        let events = match broker_events::reconcile(
            &current,
            &owner,
            &broker,
            &trades,
            &self.factory,
            Self::now(),
        ) {
            Ok(events) => events,
            Err(e) => return skip(format!("{e}")),
        };
        let mut order = current.clone();
        let mut filled = 0_i64;
        let mut trade_ids = Vec::new();
        for event in &events {
            if let OrderEventAny::Filled(f) = event {
                let Some(qty) = f.last_qty.as_decimal().to_i64() else {
                    return skip("invalid fill quantity".into());
                };
                filled += qty;
                trade_ids.push(f.trade_id.to_string());
            }
            if order.apply(event.clone()).is_err() {
                return skip("fill does not apply to the native order".into());
            }
        }
        if order.status() != OrderStatus::Filled || trade_ids.is_empty() {
            return skip("observation does not complete the order".into());
        }
        let position = self.position
            + if order.order_side() == OrderSide::Buy {
                filled
            } else {
                -filled
            };
        if position.abs() > self.max_lots {
            return skip("resulting position exceeds contract cap".into());
        }
        let mut next = record.clone();
        next.broker_id = Some(broker_id.to_owned());
        next.outcome = "Observed".into();
        next.events.extend(events.iter().cloned());
        // From here a failed write or delivery poisons the dispatcher, as in `refresh`.
        self.poisoned = true;
        self.clean = None;
        self.store.save(&id, &next)?;
        self.records.insert(id.clone(), next);
        self.position = position;
        self.verifying
            .insert(id.clone(), (trade_ids, Instant::now() + self.verify_grace));
        for event in events {
            Self::emit(tx, event)?;
        }
        self.poisoned = false;
        self.postback_fills += 1;
        eprintln!(
            "{}",
            serde_json::json!({"event":"native_postback_fill","order":id,"qty":filled,
                "position":position,"ms":started.elapsed().as_millis() as u64})
        );
        Ok(true)
    }
    fn prepare_observation(
        &self,
        snapshot: &super::broker::Snapshot,
    ) -> Result<PreparedObservation> {
        let positions = super::reports::positions_for(
            snapshot,
            self.factory.account_id(),
            &self.product,
            self.token,
            &self.instrument_id,
            &self.symbol,
            Self::now(),
        )?;
        use rust_decimal::prelude::ToPrimitive;
        let quantity = positions[0]
            .quantity
            .as_decimal()
            .to_i64()
            .ok_or_else(|| anyhow!("Invalid account position"))?;
        let position = if positions[0].position_side == nautilus_model::enums::PositionSide::Short {
            -quantity
        } else {
            quantity
        };
        ensure!(position.abs() <= self.max_lots, "Account position exceeds contract cap");
        ensure!(
            snapshot
                .positions
                .iter()
                .filter(|p| p.quantity != 0)
                .all(|p| p.exchange == "MCX"
                    && p.tradingsymbol == self.symbol
                    && p.product == self.product
                    && p.instrument_token == self.token),
            "Unmanaged account exposure during reconciliation"
        );
        ensure!(
            snapshot
                .orders
                .iter()
                .filter(|o| !matches!(o.status.as_str(), "COMPLETE" | "CANCELLED" | "REJECTED"))
                .all(|o| self
                    .records
                    .values()
                    .any(|r| o.tag.as_deref() == Some(r.tag.as_str()))),
            "Unowned open account order; review required"
        );
        let mut prepared = PreparedObservation {
            position,
            records: BTreeMap::new(),
            events: Vec::new(),
            pending: false,
            verified: Vec::new(),
        };
        for (id, record) in &self.records {
            // A postback fill is checked only once all its trades are visible here; until
            // then the record is left as it is, and only inside its grace period.
            let verifying = match self.verifying.get(id) {
                Some((trades, deadline)) => {
                    let visible = trades
                        .iter()
                        .all(|t| snapshot.trades.iter().any(|s| &s.trade_id == t));
                    if !visible {
                        ensure!(
                            Instant::now() < *deadline,
                            "Postback fill not confirmed by Kite trades within grace; review required"
                        );
                        prepared.pending = true;
                        continue;
                    }
                    true
                }
                None => false,
            };
            let current = OrderAny::from_events(record.events.clone())?;
            let matches: Vec<_> = snapshot
                .orders
                .iter()
                .filter(|b| b.tag.as_deref() == Some(record.tag.as_str()))
                .collect();
            if let Some(existing) = &record.broker_id {
                ensure!(
                    snapshot
                        .orders
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
                &snapshot.trades,
                &self.factory,
                Self::now(),
            )?;
            // `reconcile` has just proved every previously emitted fill of this order is
            // present in `/trades` with the same quantity, price, time and venue id.
            if verifying {
                prepared.verified.push(id.clone());
            }
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

        let mut expected = rust_decimal::Decimal::ZERO;
        for (id, record) in &self.records {
            let record = prepared.records.get(id).unwrap_or(record);
            let order = OrderAny::from_events(record.events.clone())?;
            expected += if order.order_side() == nautilus_model::enums::OrderSide::Buy {
                order.filled_qty().as_decimal()
            } else {
                -order.filled_qty().as_decimal()
            };
        }
        if expected != rust_decimal::Decimal::from(position) && prepared.pending {
            // Positions lag a postback fill whose trades are not visible yet either: keep
            // the native position; the deadline above bounds how long this may last.
            prepared.position = self.position;
            return Ok(prepared);
        }
        if expected != rust_decimal::Decimal::from(position) {
            ensure!(
                self.has_unresolved(),
                "Broker position differs from owned fills; manual review required"
            );
            return Err(anyhow!(broker_events::ObservationLag(
                "Account exposure differs from observed owned trades"
            )));
        }
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
        ensure!(!self.poisoned, "Native dispatcher requires manual recovery");
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
        // A fresh clean reconciliation already confirmed this stop (accepted, owned, at the
        // recorded trigger) and nothing has changed at the broker since; otherwise ask REST.
        if !self.cached_fresh() {
            let snapshot = self.snapshot().await?;
            ensure!(
                snapshot.orders.iter().any(|o| o.order_id == broker_id
                    && o.status == "TRIGGER PENDING"
                    && o.tag.as_deref() == Some(record.tag.as_str())
                    && o.trigger_price == Some(rust_decimal::Decimal::from(old))),
                "Protective stop not confirmed at broker"
            );
        }
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
        self.clean = None;
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
        ensure!(!self.poisoned, "Native dispatcher requires manual recovery");
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
        self.clean = None;
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

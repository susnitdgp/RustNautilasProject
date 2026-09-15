//! Serial native command dispatch. Only the mock factory constructs this service.
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
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::mpsc::UnboundedSender;
pub(crate) struct Dispatcher {
    broker: Arc<dyn Broker>,
    store: Box<dyn Store>,
    factory: OrderEventFactory,
    product: String,
    token: u32,
    records: BTreeMap<String, Record>,
    poisoned: bool,
    position: i64,
}
impl Dispatcher {
    pub fn unresolved(&self) -> usize {
        self.records
            .values()
            .filter(|r| OrderAny::from_events(r.events.clone()).map_or(true, |o| !o.is_closed()))
            .count()
    }
    pub fn fault(&mut self) {
        self.poisoned = true;
        let _ = self
            .store
            .health("ReviewRequired", self.unresolved(), self.position);
    }
    pub fn heartbeat(&mut self) -> Result<()> {
        self.store.health(
            if self.poisoned {
                "ReviewRequired"
            } else {
                "Running"
            },
            self.unresolved(),
            self.position,
        )
    }
    pub async fn finish(
        &mut self,
        tx: &UnboundedSender<ExecutionEvent>,
        failed: bool,
    ) -> Result<()> {
        if !self.poisoned && !failed && self.refresh(tx).await.is_err() {
            self.fault();
        }
        let clean = !failed && !self.poisoned && !self.has_unresolved() && self.position == 0;
        self.store.finish(clean, self.unresolved(), self.position)?;
        if !clean {
            self.poisoned = true;
        }
        ensure!(
            clean,
            "Shutdown requires review: unresolved orders, exposure or failed reconciliation"
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
    ) -> Self {
        Self {
            broker,
            store,
            factory,
            product,
            token,
            records: BTreeMap::new(),
            poisoned: false,
            position: 0,
        }
    }
    fn now() -> UnixNanos {
        nautilus_core::time::get_atomic_clock_realtime().get_time_ns()
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
        ensure!(!self.poisoned, "Native dispatcher requires manual recovery");
        let id = order.client_order_id().to_string();
        ensure!(
            !self.records.contains_key(&id),
            "Native order was already attempted; no resubmission"
        );
        let tag = UUID4::new().to_string().replace('-', "")[..20].to_owned();
        let preflight = async {
            ensure!(
                self.records
                    .values()
                    .all(|r| OrderAny::from_events(r.events.clone()).is_ok_and(|o| o.is_closed())),
                "Another native order remains unresolved"
            );
            let snapshot = self.snapshot().await?;
            ensure!(
                snapshot
                    .positions
                    .iter()
                    .filter(|p| p.quantity != 0)
                    .all(|p| p.exchange == "MCX"
                        && p.tradingsymbol == "CRUDEOIL26SEPFUT"
                        && p.product == self.product
                        && p.instrument_token == self.token),
                "Unmanaged account exposure"
            );
            let reports = super::reports::positions(
                &snapshot,
                self.factory.account_id(),
                &self.product,
                self.token,
                Self::now(),
            )?;
            let actual = reports[0].quantity.as_decimal();
            let signed = if reports[0].position_side == nautilus_model::enums::PositionSide::Short {
                -actual
            } else {
                actual
            };
            ensure!(
                signed == rust_decimal::Decimal::from(position),
                "Broker and native position differ"
            );
            ensure!(
                snapshot
                    .orders
                    .iter()
                    .all(|b| matches!(b.status.as_str(), "COMPLETE" | "CANCELLED" | "REJECTED")),
                "Broker has an unresolved order for this contract"
            );
            ensure!(
                order.quantity().as_decimal() == rust_decimal::Decimal::ONE && position.abs() <= 1,
                "Native account contract cap exceeded"
            );
            ensure!(
                if order.is_reduce_only() {
                    position != 0
                } else {
                    position == 0
                },
                "Exposure requires reducing exit before another entry"
            );
            native::submit_with_position(&order, &self.product, &tag, position)
        }
        .await;
        let command = match preflight {
            Ok(c) => c,
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
        self.store.reserve()?;
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
        self.store
            .health("Dispatching", self.unresolved(), self.position)?;
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
        let snapshot = self.snapshot().await?;
        let positions = super::reports::positions(
            &snapshot,
            self.factory.account_id(),
            &self.product,
            self.token,
            Self::now(),
        )?;
        use rust_decimal::prelude::ToPrimitive;
        let quantity = positions[0]
            .quantity
            .as_decimal()
            .to_i64()
            .ok_or_else(|| anyhow!("Invalid account position"))?;
        self.position = if positions[0].position_side == nautilus_model::enums::PositionSide::Short
        {
            -quantity
        } else {
            quantity
        };
        ensure!(
            self.position.abs() <= 1,
            "Account position exceeds contract cap"
        );
        ensure!(
            snapshot
                .positions
                .iter()
                .filter(|p| p.quantity != 0)
                .all(|p| p.exchange == "MCX"
                    && p.tradingsymbol == "CRUDEOIL26SEPFUT"
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
        let mut observed_exposure = rust_decimal::Decimal::ZERO;
        for r in self.records.values() {
            let current = OrderAny::from_events(r.events.clone())?;
            let qty = snapshot
                .orders
                .iter()
                .find(|o| o.tag.as_deref() == Some(r.tag.as_str()))
                .map_or(current.filled_qty().as_decimal(), |o| {
                    rust_decimal::Decimal::from(o.filled_quantity)
                });
            observed_exposure += if current.order_side() == nautilus_model::enums::OrderSide::Buy {
                qty
            } else {
                -qty
            };
        }
        ensure!(
            observed_exposure == rust_decimal::Decimal::from(self.position),
            "Account exposure differs from observed owned trades"
        );
        for (id, record) in &mut self.records {
            let current = OrderAny::from_events(record.events.clone())?;
            let matches: Vec<_> = snapshot
                .orders
                .iter()
                .filter(|b| b.tag.as_deref() == Some(record.tag.as_str()))
                .collect();
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
            let events = broker_events::reconcile(
                &current,
                &owner,
                broker,
                &snapshot.trades,
                &self.factory,
                Self::now(),
            )?;
            if record.broker_id.as_deref() != Some(broker.order_id.as_str()) || !events.is_empty() {
                let mut next = record.clone();
                next.broker_id = Some(broker.order_id.clone());
                next.outcome = "Observed".into();
                next.events.extend(events.iter().cloned());
                self.poisoned = true;
                self.store.save(id, &next)?;
                *record = next;
                self.poisoned = false;
                for event in events {
                    Self::emit(tx, event)?;
                }
            }
        }

        let mut expected = rust_decimal::Decimal::ZERO;
        for r in self.records.values() {
            let o = OrderAny::from_events(r.events.clone())?;
            expected += if o.order_side() == nautilus_model::enums::OrderSide::Buy {
                o.filled_qty().as_decimal()
            } else {
                -o.filled_qty().as_decimal()
            };
        }
        ensure!(
            expected == rust_decimal::Decimal::from(self.position),
            "Broker position differs from owned fills; manual review required"
        );
        self.heartbeat()?;
        Ok(())
    }
    pub async fn cancel(
        &mut self,
        id: ClientOrderId,
        command_id: UUID4,
        tx: &UnboundedSender<ExecutionEvent>,
    ) -> Result<()> {
        ensure!(!self.poisoned, "Native dispatcher requires manual recovery");
        self.store.reserve()?;
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
        self.store.health("Dispatching", 1, self.position)?;
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

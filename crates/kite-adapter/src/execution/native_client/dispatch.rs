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
}
impl Dispatcher {
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
            let snapshot = self.broker.snapshot().await?;
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
                    .filter(|b| b.exchange == "MCX" && b.tradingsymbol == "CRUDEOIL26SEPFUT")
                    .all(|b| matches!(b.status.as_str(), "COMPLETE" | "CANCELLED" | "REJECTED")),
                "Broker has an unresolved order for this contract"
            );
            native::submit_with_position(&order, &self.product, &tag, position)
        }
        .await;
        let command = match preflight {
            Ok(c) => c,
            Err(_) => {
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
        let response = self.broker.execute(&command).await;
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
        // Acknowledgement alone emits neither Accepted nor Filled.
        Ok(())
    }
    pub async fn refresh(&mut self, tx: &UnboundedSender<ExecutionEvent>) -> Result<()> {
        ensure!(!self.poisoned, "Native dispatcher requires manual recovery");
        let snapshot = self.broker.snapshot().await?;
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
        Ok(())
    }
    pub async fn cancel(
        &mut self,
        id: ClientOrderId,
        command_id: UUID4,
        tx: &UnboundedSender<ExecutionEvent>,
    ) -> Result<()> {
        ensure!(!self.poisoned, "Native dispatcher requires manual recovery");
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
        let response = self.broker.execute(&command).await;
        let rejected = matches!(response, Ok(Outcome::Rejected));
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

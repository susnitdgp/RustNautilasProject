use crate::model::{Event, Intent, valid_id};
use anyhow::{Result, anyhow, bail, ensure};
use serde::Serialize;
use std::collections::BTreeMap;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    Prepared,
    Dispatching,
    Unknown,
    Accepted,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}
#[derive(Debug, Clone)]
pub struct Order {
    pub intent: Intent,
    pub status: Status,
    pub broker_id: Option<String>,
    pub filled: u32,
    trades: BTreeMap<String, (u32, i64)>,
}
#[derive(Default, Clone)]
pub struct State {
    pub(crate) orders: BTreeMap<String, Order>,
}
impl State {
    pub fn order(&self, id: &str) -> Result<&Order> {
        self.orders
            .get(id)
            .ok_or_else(|| anyhow!("Unknown local intent"))
    }
    pub fn orders(&self) -> impl Iterator<Item = &Order> {
        self.orders.values()
    }
    /// Returns false for exact duplicate acknowledgements/fills; rejects conflicting duplicates.
    pub(crate) fn apply(&mut self, event: &Event) -> Result<bool> {
        if let Event::Intent { intent } = event {
            intent.validate()?;
            ensure!(
                !self.orders.contains_key(&intent.id),
                "Intent ID already exists; submission blocked"
            );
            self.orders.insert(
                intent.id.clone(),
                Order {
                    intent: intent.clone(),
                    status: Status::Prepared,
                    broker_id: None,
                    filled: 0,
                    trades: BTreeMap::new(),
                },
            );
            return Ok(true);
        }
        let binding = match event {
            Event::Acknowledged { broker_id, .. }
            | Event::Fill { broker_id, .. }
            | Event::Cancelled { broker_id, .. } => Some(broker_id),
            _ => None,
        };
        if let Some(broker_id) = binding {
            ensure!(valid_id(broker_id), "Invalid simulated broker ID");
            ensure!(
                !self
                    .orders
                    .values()
                    .any(|o| o.intent.id != event.id() && o.broker_id.as_ref() == Some(broker_id)),
                "Broker ID already belongs to another intent"
            );
        }
        let order = self
            .orders
            .get_mut(event.id())
            .ok_or_else(|| anyhow!("Unknown local intent"))?;
        if let Some(broker_id) = binding {
            if let Some(existing) = &order.broker_id {
                ensure!(existing == broker_id, "Conflicting broker identity");
            } else {
                ensure!(
                    matches!(order.status, Status::Dispatching | Status::Unknown),
                    "Broker event before dispatch or after rejection"
                );
                order.broker_id = Some(broker_id.clone());
            }
        }
        match event {
            Event::Dispatch { .. } => {
                ensure!(
                    order.status == Status::Prepared,
                    "Intent already attempted; blind retry blocked"
                );
                order.status = Status::Dispatching;
            }
            Event::Unknown { .. } => {
                if order.broker_id.is_some() || order.status == Status::Unknown {
                    return Ok(false);
                }
                ensure!(
                    order.status == Status::Dispatching,
                    "Unexpected ambiguous response"
                );
                order.status = Status::Unknown;
            }
            Event::Acknowledged { .. } => {
                if matches!(
                    order.status,
                    Status::Accepted | Status::PartiallyFilled | Status::Filled | Status::Cancelled
                ) {
                    return Ok(false);
                }
                ensure!(
                    matches!(order.status, Status::Dispatching | Status::Unknown),
                    "Unexpected acknowledgement"
                );
                order.status = Status::Accepted;
            }
            Event::Rejected { .. } => {
                if order.status == Status::Rejected {
                    return Ok(false);
                }
                ensure!(
                    matches!(order.status, Status::Dispatching | Status::Unknown)
                        && order.broker_id.is_none(),
                    "Rejection conflicts with known broker order"
                );
                order.status = Status::Rejected;
            }
            Event::Fill {
                trade_id,
                quantity,
                price_paise,
                ..
            } => {
                ensure!(
                    valid_id(trade_id)
                        && *quantity > 0
                        && *price_paise > 0
                        && price_paise % 100 == 0,
                    "Invalid simulated fill"
                );
                if let Some(previous) = order.trades.get(trade_id) {
                    ensure!(
                        *previous == (*quantity, *price_paise),
                        "Conflicting duplicate trade"
                    );
                    return Ok(false);
                }
                ensure!(
                    !matches!(
                        order.status,
                        Status::Prepared | Status::Rejected | Status::Filled
                    ),
                    "Fill conflicts with order state"
                );
                let filled = order
                    .filled
                    .checked_add(*quantity)
                    .ok_or_else(|| anyhow!("Fill quantity overflow"))?;
                ensure!(filled <= order.intent.quantity, "Overfill rejected");
                match order.intent.side {
                    crate::model::Side::Buy => ensure!(
                        *price_paise <= order.intent.limit_price_paise,
                        "Buy fill violates limit"
                    ),
                    crate::model::Side::Sell => ensure!(
                        *price_paise >= order.intent.limit_price_paise,
                        "Sell fill violates limit"
                    ),
                }
                order
                    .trades
                    .insert(trade_id.clone(), (*quantity, *price_paise));
                order.filled = filled;
                order.status = if filled == order.intent.quantity {
                    Status::Filled
                } else if order.status == Status::Cancelled {
                    Status::Cancelled
                } else {
                    Status::PartiallyFilled
                };
            }
            Event::Cancelled { .. } => {
                if order.status == Status::Cancelled {
                    return Ok(false);
                }
                ensure!(
                    !matches!(
                        order.status,
                        Status::Prepared | Status::Rejected | Status::Filled
                    ),
                    "Cancellation conflicts with terminal order"
                );
                order.status = Status::Cancelled;
            }
            Event::Intent { .. } => bail!("Unexpected intent"),
        }
        Ok(true)
    }
}

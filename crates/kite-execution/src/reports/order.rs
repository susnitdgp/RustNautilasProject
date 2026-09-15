use super::identity;
use anyhow::{Result, anyhow};
use kite_journal::{
    model::{Event, Side},
    state::{Order, Status},
    store::Record,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::{OrderSide, OrderStatus, OrderType, TimeInForce},
    identifiers::{ClientOrderId, VenueOrderId},
    reports::OrderStatusReport,
    types::{Price, Quantity},
};
use rust_decimal::Decimal;
use std::str::FromStr;
pub fn map(
    order: &Order,
    history: &[Record],
    generation: &str,
    ts_init: u64,
) -> Result<Option<OrderStatusReport>> {
    if order
        .commands
        .entries
        .values()
        .any(|c| !c.status.terminal())
    {
        return Ok(None);
    }
    let status = match order.status {
        // The existing acknowledgement confirms OMS receipt, not venue acceptance.
        Status::Accepted => OrderStatus::Submitted,
        Status::PartiallyFilled => OrderStatus::PartiallyFilled,
        Status::Filled => OrderStatus::Filled,
        Status::Cancelled => OrderStatus::Canceled,
        Status::Prepared | Status::Dispatching | Status::Unknown | Status::Rejected => {
            return Ok(None);
        }
    };
    let broker_id = order
        .broker_id
        .as_ref()
        .ok_or_else(|| anyhow!("Report requires established broker identity"))?;
    let last = history
        .iter()
        .enumerate()
        .rfind(|(_, r)| r.event.id() == order.intent.id)
        .ok_or_else(|| anyhow!("Report history missing"))?;
    let mut sum = Decimal::ZERO;
    let mut quantity = 0_u32;
    for r in history.iter().filter(|r| r.event.id() == order.intent.id) {
        if let Event::Fill {
            quantity: q,
            price_paise,
            ..
        } = r.event
        {
            quantity += q;
            sum += Decimal::from(q) * Decimal::new(price_paise, 2);
        }
    }
    let mut report = OrderStatusReport::new(
        identity::account(),
        identity::instrument(),
        Some(ClientOrderId::from(order.intent.id.as_str())),
        VenueOrderId::from(broker_id.as_str()),
        Some(match order.intent.side {
            Side::Buy => OrderSide::Buy,
            Side::Sell => OrderSide::Sell,
        }),
        OrderType::Limit,
        TimeInForce::Day,
        status,
        Quantity::from(order.intent.quantity),
        Quantity::from(order.filled),
        UnixNanos::from(0),
        UnixNanos::from(last.1.recorded_at_ns),
        UnixNanos::from(ts_init),
        Some(identity::report_id(generation, "order", last.0 + 1)),
    );
    report.price = Some(
        Price::from_str(
            &Decimal::new(order.intent.limit_price_paise, 2)
                .normalize()
                .to_string(),
        )
        .map_err(anyhow::Error::msg)?,
    );
    if quantity > 0 {
        report.avg_px = Some(sum / Decimal::from(quantity));
    }
    Ok(Some(report))
}

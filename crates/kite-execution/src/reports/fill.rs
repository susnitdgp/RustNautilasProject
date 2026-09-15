use super::identity;
use anyhow::{Result, ensure};
use kite_journal::{
    model::{Event, Side},
    state::Order,
    store::Record,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::{LiquiditySide, OrderSide},
    identifiers::{ClientOrderId, TradeId, VenueOrderId},
    reports::FillReport,
    types::{Currency, Money, Price, Quantity},
};
use rust_decimal::Decimal;
use std::str::FromStr;
pub fn map(
    record: &Record,
    order: &Order,
    generation: &str,
    sequence: usize,
    ts_init: u64,
) -> Result<Option<FillReport>> {
    let Event::Fill {
        id,
        broker_id,
        trade_id,
        quantity,
        price_paise,
    } = &record.event
    else {
        return Ok(None);
    };
    ensure!(
        id == &order.intent.id && order.broker_id.as_ref() == Some(broker_id),
        "Fill report identity mismatch"
    );
    let price = Price::from_str(&Decimal::new(*price_paise, 2).normalize().to_string())
        .map_err(anyhow::Error::msg)?;
    Ok(Some(FillReport::new(
        identity::account(),
        identity::instrument(),
        VenueOrderId::from(broker_id.as_str()),
        TradeId::from(trade_id.as_str()),
        match order.intent.side {
            Side::Buy => OrderSide::Buy,
            Side::Sell => OrderSide::Sell,
        },
        Quantity::from(*quantity),
        price,
        // Simulation-only fixture: no commission data was supplied by a broker.
        Money::new(0.0, Currency::from_str("INR")?),
        LiquiditySide::NoLiquiditySide,
        Some(ClientOrderId::from(id.as_str())),
        None,
        UnixNanos::from(record.recorded_at_ns),
        UnixNanos::from(ts_init),
        Some(identity::report_id(generation, "fill", sequence)),
    )))
}

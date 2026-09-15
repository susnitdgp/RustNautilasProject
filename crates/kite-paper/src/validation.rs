use anyhow::{Result, ensure};
use kite_strategy::config::Config;
use nautilus_model::{
    data::QuoteTick,
    enums::{OrderSide, OrderType, TimeInForce},
    events::OrderInitialized,
    identifiers::InstrumentId,
    types::Quantity,
};
use rust_decimal::Decimal;
pub fn order(o: &OrderInitialized) -> Result<()> {
    ensure!(
        o.instrument_id == InstrumentId::from("CRUDEOIL26SEPFUT.MCX")
            && o.order_type == OrderType::Limit
            && o.time_in_force == TimeInForce::Day
            && o.quantity == Quantity::from(1),
        "Paper supports one-contract MCX crude LIMIT DAY only"
    );
    ensure!(
        matches!(o.order_side, OrderSide::Buy | OrderSide::Sell)
            && !o.post_only
            && !o.reduce_only
            && !o.quote_quantity
            && o.exec_algorithm_id.is_none()
            && o.linked_order_ids.is_none()
            && o.parent_order_id.is_none()
            && o.trigger_price.is_none()
            && o.display_qty.is_none(),
        "Unsupported paper order flags"
    );
    let price = o
        .price
        .ok_or_else(|| anyhow::anyhow!("Missing paper limit"))?
        .as_decimal();
    ensure!(
        price > Decimal::ZERO && price.fract().is_zero(),
        "Invalid paper tick"
    );
    let id = o.client_order_id.to_string();
    ensure!(
        !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_alphanumeric()),
        "Invalid paper order ID"
    );
    Ok(())
}
pub fn quote(q: &QuoteTick, config: &Config, last_ts: u64) -> Result<()> {
    let bid = q.bid_price.as_decimal();
    let ask = q.ask_price.as_decimal();
    ensure!(
        q.instrument_id == InstrumentId::from("CRUDEOIL26SEPFUT.MCX")
            && bid > Decimal::ZERO
            && ask >= bid
            && bid.fract().is_zero()
            && ask.fract().is_zero(),
        "Invalid paper quote prices"
    );
    ensure!(
        ask - bid <= Decimal::from(config.max_spread_rupees)
            && q.bid_size >= Quantity::from(1)
            && q.ask_size >= Quantity::from(1),
        "Paper quote liquidity rejected"
    );
    let source = q.ts_event.as_u64();
    let received = q.ts_init.as_u64();
    ensure!(
        source > last_ts
            && source.saturating_sub(received) <= 2_000_000_000
            && received.saturating_sub(source) <= u64::from(config.max_age_seconds) * 1_000_000_000,
        "Paper quote timestamp rejected"
    );
    Ok(())
}

use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{enums::*, events::*, identifiers::*, types::*};

pub fn account_id() -> AccountId {
    AccountId::from("KITE-PAPER")
}
pub fn client_id() -> ClientId {
    ClientId::from("KITE-PAPER")
}
pub fn accepted(o: &OrderInitialized, venue: VenueOrderId, ts: UnixNanos) -> Vec<OrderEventAny> {
    vec![
        OrderEventAny::Submitted(OrderSubmitted::new(
            o.trader_id,
            o.strategy_id,
            o.instrument_id,
            o.client_order_id,
            account_id(),
            UUID4::new(),
            ts,
            ts,
        )),
        OrderEventAny::Accepted(OrderAccepted::new(
            o.trader_id,
            o.strategy_id,
            o.instrument_id,
            o.client_order_id,
            venue,
            account_id(),
            UUID4::new(),
            ts,
            ts,
            false,
        )),
    ]
}
pub fn filled(
    o: &OrderInitialized,
    venue: VenueOrderId,
    price: Price,
    ts: UnixNanos,
) -> OrderEventAny {
    OrderEventAny::Filled(OrderFilled::new(
        o.trader_id,
        o.strategy_id,
        o.instrument_id,
        o.client_order_id,
        venue,
        account_id(),
        TradeId::from(format!("T{}", o.client_order_id).as_str()),
        o.order_side,
        o.order_type,
        o.quantity,
        price,
        Currency::INR(),
        LiquiditySide::Taker,
        UUID4::new(),
        ts,
        ts,
        false,
        None,
        Some(Money::new(0.0, Currency::INR())),
        None,
    ))
}
pub fn cancelled(o: &OrderInitialized, venue: VenueOrderId, ts: UnixNanos) -> OrderEventAny {
    OrderEventAny::Canceled(OrderCanceled::new(
        o.trader_id,
        o.strategy_id,
        o.instrument_id,
        o.client_order_id,
        UUID4::new(),
        ts,
        ts,
        false,
        Some(venue),
        Some(account_id()),
    ))
}

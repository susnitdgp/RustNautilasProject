//! Exact native-order translation into the existing guarded Kite request type.
//! No network access here. The native dispatcher persists the client-order-ID
//! to broker-tag association before submission.
use super::request::Command;
use anyhow::{Result, anyhow, ensure};
use nautilus_model::{
    enums::{OrderSide, OrderType, TimeInForce},
    identifiers::InstrumentId,
    orders::{Order, OrderAny},
};
use rust_decimal::prelude::ToPrimitive;
pub fn submit(order: &OrderAny, product: &str, persisted_tag: &str) -> Result<Command> {
    translate(order, product, persisted_tag, false)
}
pub(crate) fn submit_with_position(
    order: &OrderAny,
    product: &str,
    tag: &str,
    position: i64,
) -> Result<Command> {
    if order.is_reduce_only() {
        let opposing = matches!(
            (order.order_side(), position.signum()),
            (OrderSide::Buy, -1) | (OrderSide::Sell, 1)
        );
        ensure!(
            opposing
                && order.quantity().as_decimal()
                    <= rust_decimal::Decimal::from(position.unsigned_abs()),
            "Reduce-only order would increase or reverse exposure"
        );
    }
    translate(order, product, tag, true)
}
fn translate(
    order: &OrderAny,
    product: &str,
    persisted_tag: &str,
    reducing_policy_checked: bool,
) -> Result<Command> {
    ensure!(
        order.instrument_id() == InstrumentId::from("CRUDEOIL26SEPFUT.MCX"),
        "Unsupported native instrument"
    );
    ensure!(
        !order.is_closed(),
        "Closed native order cannot be submitted"
    );
    ensure!(
        order.order_type() == OrderType::Limit && order.time_in_force() == TimeInForce::Day,
        "Native Kite submission supports regular LIMIT/DAY only"
    );
    ensure!(
        !order.is_post_only()
            && (!order.is_reduce_only() || reducing_policy_checked)
            && !order.is_quote_quantity(),
        "Native order instruction requires an execution policy not yet integrated"
    );
    ensure!(
        order.contingency_type().is_none() && order.emulation_trigger().is_none(),
        "Contingent or emulated order must be resolved before Kite submission"
    );
    let quantity = order.quantity().as_decimal();
    let price = order
        .price()
        .ok_or_else(|| anyhow!("Native limit price missing"))?
        .as_decimal();
    ensure!(
        quantity.fract().is_zero() && price.fract().is_zero(),
        "Native quantity and rupee price must be exact integers for this contract"
    );
    let command = Command::Place {
        symbol: "CRUDEOIL26SEPFUT".into(),
        side: match order.order_side() {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        }
        .into(),
        product: product.into(),
        quantity: quantity
            .to_u32()
            .ok_or_else(|| anyhow!("Native quantity out of range"))?,
        price_rupees: price
            .to_i64()
            .ok_or_else(|| anyhow!("Native price out of range"))?,
        tag: persisted_tag.into(),
    };
    command.validate()?;
    Ok(command)
}
#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_common::{clock::TestClock, factories::OrderFactory};
    use nautilus_model::types::{Price, Quantity};
    use std::{cell::RefCell, rc::Rc};
    fn factory() -> OrderFactory {
        OrderFactory::new(
            "SUSANTA-001".into(),
            "CROSSOVER-001".into(),
            None,
            None,
            Rc::new(RefCell::new(TestClock::new())),
            false,
            false,
        )
    }
    fn limit(quantity: Quantity, price: Price, reduce_only: bool) -> OrderAny {
        factory().limit(
            "CRUDEOIL26SEPFUT.MCX".into(),
            OrderSide::Buy,
            quantity,
            price,
            Some(TimeInForce::Day),
            None,
            None,
            Some(reduce_only),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
    }
    #[test]
    fn native_limit_preserves_exact_terms_and_explicit_correlation_tag() {
        let order = limit(Quantity::from(2), Price::new(6001.0, 0), false);
        match submit(&order, "NRML", "KiteNative0001").unwrap() {
            Command::Place {
                symbol,
                side,
                product,
                quantity,
                price_rupees,
                tag,
            } => {
                assert_eq!(symbol, "CRUDEOIL26SEPFUT");
                assert_eq!(side, "BUY");
                assert_eq!(product, "NRML");
                assert_eq!(quantity, 2);
                assert_eq!(price_rupees, 6001);
                assert_eq!(tag, "KiteNative0001");
            }
            _ => panic!("Expected place request"),
        }
    }
    #[test]
    fn rejects_fractional_terms_instead_of_truncating() {
        assert!(
            submit(
                &limit(Quantity::from("1.5"), Price::new(6000.0, 0), false),
                "NRML",
                "Test1"
            )
            .is_err()
        );
        assert!(
            submit(
                &limit(Quantity::from(1), Price::new(6000.5, 1), false),
                "NRML",
                "Test1"
            )
            .is_err()
        );
    }
    #[test]
    fn rejects_unrepresented_instruction_and_invalid_broker_fields() {
        assert!(
            submit(
                &limit(Quantity::from(1), Price::new(6000.0, 0), true),
                "NRML",
                "Test1"
            )
            .is_err()
        );
        let order = limit(Quantity::from(1), Price::new(6000.0, 0), false);
        assert!(submit(&order, "CNC", "Test1").is_err());
        assert!(submit(&order, "NRML", "unmapped-native-id-with-hyphens").is_err());
        assert!(submit(&order, "NRML", "").is_err());
        let market = factory().market(
            "CRUDEOIL26SEPFUT.MCX".into(),
            OrderSide::Buy,
            Quantity::from(1),
            Some(TimeInForce::Day),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(submit(&market, "NRML", "Test1").is_err());
    }
}

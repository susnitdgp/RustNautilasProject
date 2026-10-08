//! ILRC signal -> Nautilus native order bridge for the existing Kite dispatcher.
//! No execution client, credential or broker call is created by this module.
use super::ilrc_backtest::EntryEvent;
use anyhow::{Result, ensure};
use nautilus_common::factories::OrderFactory;
use nautilus_model::{
    enums::{OrderSide, TimeInForce},
    orders::OrderAny,
    types::Quantity,
};

pub fn entry_order(
    factory: &mut OrderFactory,
    instrument: &str,
    signal: &EntryEvent,
) -> Result<OrderAny> {
    ensure!(matches!(signal.setup, "A" | "B"), "Unknown ILRC setup");
    ensure!(
        chrono::DateTime::parse_from_rfc3339(&signal.observed_at)?
            >= chrono::DateTime::parse_from_rfc3339(&signal.entry_time)?
                + chrono::Duration::minutes(3),
        "Signal candle not finalized"
    );
    ensure!(
        [signal.entry, signal.stop, signal.target]
            .iter()
            .all(|x| x.is_finite() && *x > 0.0),
        "Invalid signal prices"
    );
    let side = match signal.side {
        "LONG" => {
            ensure!(
                signal.stop < signal.entry && signal.target > signal.entry,
                "Invalid long stop/target"
            );
            OrderSide::Buy
        }
        "SHORT" => {
            ensure!(
                signal.stop > signal.entry && signal.target < signal.entry,
                "Invalid short stop/target"
            );
            OrderSide::Sell
        }
        _ => anyhow::bail!("Invalid side"),
    };
    let symbol = kite_adapter::instruments::contract::symbol_from_instrument_id(instrument)?;
    kite_adapter::instruments::contract::validate_symbol(symbol)?;
    Ok(factory.market(
        instrument.into(),
        side,
        Quantity::from(1),
        Some(TimeInForce::Day),
        Some(false),
        Some(false),
        None,
        None,
        None,
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_common::clock::TestClock;
    use nautilus_model::{enums::OrderType, orders::Order};
    use std::{cell::RefCell, rc::Rc};
    fn factory() -> OrderFactory {
        OrderFactory::new(
            "SUSANTA-001".into(),
            "ILRC-001".into(),
            None,
            None,
            Rc::new(RefCell::new(TestClock::new())),
            false,
            false,
        )
    }
    fn signal() -> EntryEvent {
        EntryEvent {
            setup: "A",
            entry_time: "2026-10-07T12:00:00+05:30".into(),
            observed_at: "2026-10-07T12:03:00+05:30".into(),
            side: "LONG",
            entry: 8700.,
            stop: 8690.,
            target: 8730.,
        }
    }
    #[test]
    fn order_translates_through_real_native_kite_command_adapter() {
        let o = entry_order(&mut factory(), "CRUDEOIL26OCTFUT.MCX", &signal()).unwrap();
        assert_eq!(o.order_type(), OrderType::Market);
        assert!(!o.is_reduce_only());
        let cmd = kite_adapter::execution::native::submit(&o, "MIS", "ILRCTEST0001").unwrap();
        assert!(matches!(
            cmd,
            kite_adapter::execution::request::Command::ProtectedMarket {
                quantity: 1,
                market_protection: -1,
                ..
            }
        ));
    }
    #[test]
    fn malformed_signal_rejected() {
        let mut s = signal();
        s.stop = 8800.;
        assert!(entry_order(&mut factory(), "CRUDEOIL26OCTFUT.MCX", &s).is_err());
    }
}

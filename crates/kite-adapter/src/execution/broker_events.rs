//! Broker observations to native events. HTTP acknowledgement is never acceptance.
//! Kite /trades supplies no commission: native fills retain `commission: None`.
use anyhow::{Result, anyhow, bail, ensure};
use chrono::{FixedOffset, NaiveDateTime, TimeZone};
use nautilus_common::factories::OrderEventFactory;
use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::{LiquiditySide, OrderSide, OrderStatus},
    events::OrderEventAny,
    identifiers::{TradeId, VenueOrderId},
    orders::{Order, OrderAny},
    types::{Currency, Price, Quantity},
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::{collections::BTreeSet, str::FromStr};

/// Separately fetched broker views have not converged. No events may be applied
/// from this observation; only the bounded read path may retry it.
#[derive(Debug)]
pub(crate) struct ObservationLag(pub &'static str);
impl std::fmt::Display for ObservationLag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ObservationLag {}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct BrokerOrder {
    pub order_id: String,
    pub exchange: String,
    pub tradingsymbol: String,
    pub instrument_token: u32,
    pub product: String,
    pub transaction_type: String,
    pub variety: String,
    pub order_type: String,
    #[serde(default)]
    pub market_protection: Option<Decimal>,
    pub validity: String,
    pub status: String,
    pub quantity: u32,
    pub filled_quantity: u32,
    pub price: Decimal,
    pub tag: Option<String>,
    pub exchange_timestamp: Option<String>,
    pub exchange_update_timestamp: Option<String>,
    pub order_timestamp: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct BrokerTrade {
    pub trade_id: String,
    pub order_id: String,
    pub exchange: String,
    pub tradingsymbol: String,
    pub instrument_token: u32,
    pub product: String,
    pub transaction_type: String,
    pub quantity: u32,
    pub average_price: Decimal,
    pub fill_timestamp: String,
}
/// Correlation must originate from durable command ownership, never tag guessing.
pub struct Ownership<'a> {
    pub broker_id: &'a str,
    pub tag: &'a str,
    pub product: &'a str,
    pub token: u32,
}
pub fn timestamp(value: &str) -> Result<UnixNanos> {
    let local = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .map_err(|_| anyhow!("Invalid Kite exchange timestamp"))?;
    let utc = FixedOffset::east_opt(19_800)
        .expect("India offset")
        .from_local_datetime(&local)
        .single()
        .ok_or_else(|| anyhow!("Invalid Kite exchange time"))?;
    let nanos = utc
        .timestamp_nanos_opt()
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| anyhow!("Kite timestamp out of range"))?;
    Ok(nanos.into())
}
fn side(value: &str) -> Result<OrderSide> {
    match value {
        "BUY" => Ok(OrderSide::Buy),
        "SELL" => Ok(OrderSide::Sell),
        _ => bail!("Invalid Kite order side"),
    }
}
fn price(value: Decimal) -> Result<Price> {
    ensure!(
        value > Decimal::ZERO && value.fract().is_zero(),
        "Invalid crude oil trade price"
    );
    Price::from_str(&value.normalize().to_string()).map_err(anyhow::Error::msg)
}
fn apply(
    order: &mut OrderAny,
    events: &mut Vec<OrderEventAny>,
    event: OrderEventAny,
) -> Result<()> {
    order.apply(event.clone())?;
    events.push(event);
    Ok(())
}
/// Validate the entire observation before returning any event. Consumers must process
/// this sequence once through the native execution engine (not replace its cache).
pub fn reconcile(
    current: &OrderAny,
    owner: &Ownership<'_>,
    broker: &BrokerOrder,
    trades: &[BrokerTrade],
    factory: &OrderEventFactory,
    now: UnixNanos,
) -> Result<Vec<OrderEventAny>> {
    super::request::broker_id(owner.broker_id)?;
    ensure!(
        current.trader_id() == factory.trader_id(),
        "Native trader mismatch"
    );
    ensure!(
        current
            .account_id()
            .is_none_or(|id| id == factory.account_id()),
        "Native account mismatch"
    );
    let instrument_id = current.instrument_id().to_string();
    let symbol = crate::instruments::contract::symbol_from_instrument_id(&instrument_id)?;
    ensure!(
        current
            .venue_order_id()
            .is_none_or(|id| id.as_str() == owner.broker_id),
        "Native broker identity mismatch"
    );
    ensure!(
        broker.order_id == owner.broker_id && broker.tag.as_deref() == Some(owner.tag),
        "Kite order ownership mismatch"
    );
    ensure!(
        broker.exchange == "MCX"
            && broker.tradingsymbol == symbol
            && broker.instrument_token == owner.token
            && broker.product == owner.product,
        "Kite order contract/product mismatch"
    );
    ensure!(
        broker.variety == "regular"
            && matches!(broker.order_type.as_str(), "LIMIT" | "MARKET")
            && broker.validity == "DAY",
        "Unsupported Kite order instructions"
    );
    ensure!(
        side(&broker.transaction_type)? == current.order_side(),
        "Kite order side mismatch"
    );
    ensure!(
        broker.quantity > 0 && broker.filled_quantity <= broker.quantity,
        "Invalid Kite order quantities"
    );
    let market = current.order_type() == nautilus_model::enums::OrderType::Market;
    if market {
        ensure!(
            broker
                .market_protection
                .is_some_and(|p| p == Decimal::from(-1)
                    || (p > Decimal::ZERO && p <= Decimal::from(100))
                    || (p == Decimal::ZERO
                        && broker.order_type == "LIMIT"
                        && broker.price > Decimal::ZERO
                        && broker.price.fract().is_zero())),
            "Owned market order has unsupported broker conversion metadata"
        );
    } else {
        ensure!(broker.order_type == "LIMIT", "Broker order type changed");
    }
    // Modification requires its separately persisted ownership/intent before support.
    ensure!(
        Quantity::from(broker.quantity) == current.quantity()
            && (market || Some(price(broker.price)?) == current.price()),
        "Unconfirmed Kite order modification"
    );
    let last = timestamp(
        broker
            .exchange_update_timestamp
            .as_deref()
            .unwrap_or(&broker.order_timestamp),
    )?;
    ensure!(last <= now, "Future Kite order observation");
    let venue_id = VenueOrderId::from(owner.broker_id);
    let mut seen = BTreeSet::new();
    let mut matched = Vec::new();
    let mut sum = 0_u32;
    for trade in trades.iter().filter(|t| t.order_id == owner.broker_id) {
        super::request::broker_id(&trade.trade_id)?;
        ensure!(
            seen.insert(trade.trade_id.clone()),
            "Duplicate Kite trade identity"
        );
        ensure!(
            trade.exchange == broker.exchange
                && trade.tradingsymbol == broker.tradingsymbol
                && trade.instrument_token == owner.token
                && trade.product == owner.product
                && side(&trade.transaction_type)? == current.order_side(),
            "Kite trade ownership mismatch"
        );
        ensure!(trade.quantity > 0, "Empty Kite trade");
        let ts = timestamp(&trade.fill_timestamp)?;
        ensure!(ts <= now, "Future Kite trade observation");
        let px = price(trade.average_price)?;
        sum = sum
            .checked_add(trade.quantity)
            .ok_or_else(|| anyhow!("Kite trade quantity overflow"))?;
        matched.push((ts, trade, px));
    }
    matched.sort_by(|a, b| (a.0, &a.1.trade_id).cmp(&(b.0, &b.1.trade_id)));
    let existing: BTreeSet<String> = current
        .events()
        .iter()
        .filter_map(|e| {
            if let OrderEventAny::Filled(f) = e {
                Some(f.trade_id.to_string())
            } else {
                None
            }
        })
        .collect();
    let mut known_qty = 0_u32;
    for (ts, t, px) in &matched {
        if existing.contains(&t.trade_id) {
            let prior = current
                .events()
                .iter()
                .find_map(|e| match e {
                    OrderEventAny::Filled(f) if f.trade_id.as_str() == t.trade_id => Some(f),
                    _ => None,
                })
                .ok_or_else(|| anyhow!("Native fill history missing"))?;
            ensure!(
                prior.last_qty == Quantity::from(t.quantity)
                    && prior.last_px == *px
                    && prior.venue_order_id == venue_id
                    && prior.ts_event == *ts,
                "Kite trade changed after processing"
            );
            known_qty = known_qty
                .checked_add(t.quantity)
                .ok_or_else(|| anyhow!("Known quantity overflow"))?;
        }
    }
    ensure!(
        Quantity::from(known_qty) == current.filled_qty(),
        "Kite snapshot omits native fills"
    );
    // Check immutable native fill history first. Missing/changed previously
    // processed trades and invalid identities are integrity failures, not lag.
    ensure!(
        matched.iter().all(|(ts, _, _)| *ts <= last),
        ObservationLag("Trade ahead of order snapshot; fetch a consistent snapshot")
    );
    ensure!(
        sum == broker.filled_quantity,
        ObservationLag("Order/trade snapshot mismatch; no inferred fills")
    );
    let accepted = match broker.status.as_str() {
        "OPEN" | "COMPLETE" | "CANCELLED" => true,
        "REJECTED" => false,
        "PUT ORDER REQ RECEIVED" | "VALIDATION PENDING" | "OPEN PENDING" => {
            ensure!(
                sum == 0 && current.status() == OrderStatus::Submitted,
                "Regressing pending Kite observation"
            );
            return Ok(vec![]);
        }
        _ => bail!("Unresolved Kite order status; reconcile again"),
    };
    ensure!(
        broker.status != "COMPLETE" || sum == broker.quantity,
        "Incomplete COMPLETE observation"
    );
    ensure!(
        broker.status != "OPEN" || sum < broker.quantity,
        "Filled OPEN observation"
    );
    ensure!(
        broker.status != "REJECTED" || sum == 0,
        "Rejected order contains trades"
    );
    let mut order = current.clone();
    let mut events = vec![];
    if accepted && order.status() == OrderStatus::Submitted {
        let ts = timestamp(
            broker
                .exchange_timestamp
                .as_deref()
                .ok_or_else(|| anyhow!("Exchange acceptance timestamp missing"))?,
        )?;
        ensure!(
            ts <= last && matched.first().is_none_or(|t| ts <= t.0),
            "Invalid acceptance chronology"
        );
        let e = factory.generate_order_accepted(&order, venue_id, ts, now);
        apply(&mut order, &mut events, e)?;
    }
    for (ts, trade, px) in matched {
        if !existing.contains(&trade.trade_id) {
            let e = factory.generate_order_filled(
                &order,
                venue_id,
                None,
                TradeId::from(trade.trade_id.as_str()),
                Quantity::from(trade.quantity),
                px,
                Currency::INR(),
                None,
                LiquiditySide::NoLiquiditySide,
                ts,
                now,
            );
            apply(&mut order, &mut events, e)?;
        }
    }
    match broker.status.as_str() {
        "CANCELLED" if order.status() != OrderStatus::Canceled => {
            let e = factory.generate_order_canceled(&order, Some(venue_id), last, now);
            apply(&mut order, &mut events, e)?;
        }
        "REJECTED" if order.status() != OrderStatus::Rejected => {
            let e =
                factory.generate_order_rejected(&order, "Kite rejected order", last, now, false);
            apply(&mut order, &mut events, e)?;
        }
        "COMPLETE" => ensure!(
            order.status() == OrderStatus::Filled,
            "Native order not filled"
        ),
        "OPEN" => ensure!(
            matches!(
                order.status(),
                OrderStatus::Accepted | OrderStatus::PartiallyFilled
            ),
            "Regressing open observation"
        ),
        _ => {}
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_common::{clock::TestClock, factories::OrderFactory};
    use nautilus_model::enums::{AccountType, TimeInForce};
    use std::{cell::RefCell, rc::Rc};
    fn fixture() -> (OrderAny, OrderEventFactory, BrokerOrder, BrokerTrade) {
        let mut factory = OrderFactory::new(
            "SUSANTA-001".into(),
            "CROSSOVER-001".into(),
            None,
            None,
            Rc::new(RefCell::new(TestClock::new())),
            false,
            false,
        );
        let mut order = factory.limit(
            "CRUDEOIL26SEPFUT.MCX".into(),
            OrderSide::Buy,
            Quantity::from(2),
            Price::from("6000"),
            Some(TimeInForce::Day),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let events = OrderEventFactory::new(
            "SUSANTA-001".into(),
            "KITE-TEST".into(),
            AccountType::Margin,
            Some(Currency::INR()),
        );
        order
            .apply(
                events.generate_order_submitted(&order, timestamp("2026-09-15 10:00:00").unwrap()),
            )
            .unwrap();
        let broker = BrokerOrder {
            order_id: "123".into(),
            exchange: "MCX".into(),
            tradingsymbol: "CRUDEOIL26SEPFUT".into(),
            instrument_token: 144870151,
            product: "NRML".into(),
            transaction_type: "BUY".into(),
            variety: "regular".into(),
            order_type: "LIMIT".into(),
            market_protection: None,
            validity: "DAY".into(),
            status: "OPEN".into(),
            quantity: 2,
            filled_quantity: 1,
            price: Decimal::from(6000),
            tag: Some("Native1".into()),
            exchange_timestamp: Some("2026-09-15 10:00:01".into()),
            exchange_update_timestamp: Some("2026-09-15 10:00:03".into()),
            order_timestamp: "2026-09-15 10:00:00".into(),
        };
        let trade = BrokerTrade {
            trade_id: "456".into(),
            order_id: "123".into(),
            exchange: "MCX".into(),
            tradingsymbol: "CRUDEOIL26SEPFUT".into(),
            instrument_token: 144870151,
            product: "NRML".into(),
            transaction_type: "BUY".into(),
            quantity: 1,
            average_price: Decimal::from(5999),
            fill_timestamp: "2026-09-15 10:00:02".into(),
        };
        (order, events, broker, trade)
    }
    fn owner() -> Ownership<'static> {
        Ownership {
            broker_id: "123",
            tag: "Native1",
            product: "NRML",
            token: 144870151,
        }
    }
    fn now() -> UnixNanos {
        timestamp("2026-09-15 10:01:00").unwrap()
    }
    fn process(o: &mut OrderAny, events: Vec<OrderEventAny>) {
        for e in events {
            o.apply(e).unwrap();
        }
    }
    #[test]
    fn partial_then_complete_and_repeated_snapshot_do_not_duplicate_fills() {
        let (mut o, f, mut b, t) = fixture();
        let events = reconcile(&o, &owner(), &b, std::slice::from_ref(&t), &f, now()).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], OrderEventAny::Accepted(_)));
        match &events[1] {
            OrderEventAny::Filled(x) => {
                assert_eq!(x.last_px, Price::from("5999"));
                assert!(x.commission.is_none());
            }
            _ => panic!("fill expected"),
        }
        process(&mut o, events);
        assert_eq!(o.status(), OrderStatus::PartiallyFilled);
        assert!(
            reconcile(&o, &owner(), &b, std::slice::from_ref(&t), &f, now())
                .unwrap()
                .is_empty()
        );
        b.status = "COMPLETE".into();
        b.filled_quantity = 2;
        let mut t2 = t.clone();
        t2.trade_id = "457".into();
        t2.average_price = Decimal::from(6000);
        let trades = vec![t, t2];
        let events = reconcile(&o, &owner(), &b, &trades, &f, now()).unwrap();
        process(&mut o, events);
        assert_eq!(o.status(), OrderStatus::Filled);
        assert!(
            reconcile(&o, &owner(), &b, &trades, &f, now())
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn partial_fill_precedes_cancellation() {
        let (mut o, f, mut b, t) = fixture();
        b.status = "CANCELLED".into();
        let events = reconcile(&o, &owner(), &b, &[t], &f, now()).unwrap();
        assert_eq!(events.len(), 3);
        process(&mut o, events);
        assert_eq!(o.status(), OrderStatus::Canceled);
        assert_eq!(o.filled_qty(), Quantity::from(1));
    }
    #[test]
    fn pending_receipt_is_not_acceptance_and_rejection_needs_no_exchange_timestamp() {
        let (o, f, mut b, _) = fixture();
        b.filled_quantity = 0;
        b.status = "OPEN PENDING".into();
        b.exchange_timestamp = None;
        assert!(
            reconcile(&o, &owner(), &b, &[], &f, now())
                .unwrap()
                .is_empty()
        );
        b.status = "REJECTED".into();
        let events = reconcile(&o, &owner(), &b, &[], &f, now()).unwrap();
        assert!(matches!(&events[..], [OrderEventAny::Rejected(_)]));
    }
    #[test]
    fn inconsistent_or_duplicate_trade_snapshot_emits_nothing() {
        let (o, f, b, t) = fixture();
        assert!(reconcile(&o, &owner(), &b, &[], &f, now()).is_err());
        assert!(reconcile(&o, &owner(), &b, &[t.clone(), t.clone()], &f, now()).is_err());
        let mut ahead = t;
        ahead.fill_timestamp = "2026-09-15 10:00:04".into();
        assert!(reconcile(&o, &owner(), &b, &[ahead], &f, now()).is_err());
        assert_eq!(o.status(), OrderStatus::Submitted);
    }
    #[test]
    fn ownership_contract_side_and_modification_mismatches_fail() {
        let (o, f, b, t) = fixture();
        for field in ["tag", "product", "token", "side", "price"] {
            let mut bad = b.clone();
            match field {
                "tag" => bad.tag = None,
                "product" => bad.product = "MIS".into(),
                "token" => bad.instrument_token = 1,
                "side" => bad.transaction_type = "SELL".into(),
                _ => bad.price = Decimal::from(6001),
            }
            assert!(reconcile(&o, &owner(), &bad, std::slice::from_ref(&t), &f, now()).is_err());
        }
        let mut bad = t;
        bad.product = "MIS".into();
        assert!(reconcile(&o, &owner(), &b, &[bad], &f, now()).is_err());
    }
    #[test]
    fn changed_or_omitted_previously_processed_trade_fails() {
        let (mut o, f, mut b, mut t) = fixture();
        let events = reconcile(&o, &owner(), &b, std::slice::from_ref(&t), &f, now()).unwrap();
        process(&mut o, events);
        t.average_price = Decimal::from(5998);
        assert!(reconcile(&o, &owner(), &b, &[t], &f, now()).is_err());
        b.filled_quantity = 0;
        assert!(reconcile(&o, &owner(), &b, &[], &f, now()).is_err());
    }
    #[test]
    fn exchange_time_converts_india_to_utc_without_local_machine_timezone() {
        assert_eq!(
            timestamp("2026-09-15 05:30:00").unwrap().as_u64(),
            1789430400000000000
        );
        assert!(timestamp("10:00:00").is_err());
        assert!(timestamp("1960-01-01 00:00:00").is_err());
    }
    #[test]
    fn protected_market_conversion_requires_protection_and_handles_partial_cancel() {
        let (_, events, mut broker, trade) = fixture();
        let mut factory = OrderFactory::new(
            "SUSANTA-001".into(),
            "CROSSOVER-001".into(),
            None,
            None,
            Rc::new(RefCell::new(TestClock::new())),
            false,
            false,
        );
        let mut order = factory.market(
            "CRUDEOIL26SEPFUT.MCX".into(),
            OrderSide::Buy,
            2.into(),
            Some(TimeInForce::Day),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        order
            .apply(
                events.generate_order_submitted(&order, timestamp("2026-09-15 10:00:00").unwrap()),
            )
            .unwrap();
        assert!(
            reconcile(
                &order,
                &owner(),
                &broker,
                std::slice::from_ref(&trade),
                &events,
                now()
            )
            .is_err()
        );
        // Kite can clear protection after converting an owned market order to LIMIT.
        broker.market_protection = Some(Decimal::ZERO);
        assert!(
            reconcile(
                &order,
                &owner(),
                &broker,
                std::slice::from_ref(&trade),
                &events,
                now()
            )
            .is_ok()
        );
        broker.order_type = "MARKET".into();
        assert!(
            reconcile(
                &order,
                &owner(),
                &broker,
                std::slice::from_ref(&trade),
                &events,
                now()
            )
            .is_err()
        );
        broker.order_type = "LIMIT".into();
        broker.market_protection = Some(Decimal::from(-1));
        broker.status = "CANCELLED".into();
        let mapped = reconcile(
            &order,
            &owner(),
            &broker,
            std::slice::from_ref(&trade),
            &events,
            now(),
        )
        .unwrap();
        process(&mut order, mapped);
        assert_eq!(order.status(), OrderStatus::Canceled);
        assert_eq!(order.filled_qty(), Quantity::from(1));
    }
    #[test]
    fn observed_mis_sell_conversion_records_fill_once_and_rejects_wrong_ownership() {
        let (_, events, mut broker, mut trade) = fixture();
        let mut factory = OrderFactory::new(
            "SUSANTA-001".into(),
            "CROSSOVER-001".into(),
            None,
            None,
            Rc::new(RefCell::new(TestClock::new())),
            false,
            false,
        );
        let mut order = factory.market(
            "CRUDEOIL26SEPFUT.MCX".into(),
            OrderSide::Sell,
            1.into(),
            Some(TimeInForce::Day),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        order
            .apply(
                events.generate_order_submitted(&order, timestamp("2026-09-16 16:50:07").unwrap()),
            )
            .unwrap();
        broker.product = "MIS".into();
        broker.transaction_type = "SELL".into();
        broker.quantity = 1;
        broker.filled_quantity = 1;
        broker.status = "COMPLETE".into();
        broker.order_type = "LIMIT".into();
        broker.price = Decimal::from(9868);
        broker.market_protection = Some(Decimal::ZERO);
        broker.order_timestamp = "2026-09-16 16:50:07".into();
        broker.exchange_timestamp = Some("2026-09-16 16:50:09".into());
        broker.exchange_update_timestamp = broker.exchange_timestamp.clone();
        trade.product = "MIS".into();
        trade.transaction_type = "SELL".into();
        trade.average_price = Decimal::from(9916);
        trade.fill_timestamp = "2026-09-16 16:50:09".into();
        let owner = Ownership {
            product: "MIS",
            ..owner()
        };
        let now = timestamp("2026-09-16 16:50:10").unwrap();
        let mut foreign = broker.clone();
        foreign.tag = Some("OtherRun".into());
        assert!(
            reconcile(
                &order,
                &owner,
                &foreign,
                std::slice::from_ref(&trade),
                &events,
                now
            )
            .is_err()
        );
        let mapped = reconcile(
            &order,
            &owner,
            &broker,
            std::slice::from_ref(&trade),
            &events,
            now,
        )
        .unwrap();
        assert_eq!(mapped.len(), 2);
        assert!(matches!(&mapped[0], OrderEventAny::Accepted(_)));
        match &mapped[1] {
            OrderEventAny::Filled(fill) => {
                assert_eq!(fill.last_px, Price::from("9916"));
                assert_eq!(fill.last_qty, Quantity::from(1));
                assert_eq!(fill.order_side, OrderSide::Sell);
            }
            _ => panic!("fill required"),
        }
        process(&mut order, mapped);
        assert_eq!(order.status(), OrderStatus::Filled);
        assert!(
            reconcile(&order, &owner, &broker, &[trade], &events, now)
                .unwrap()
                .is_empty()
        );
    }
}

//! Kite virtual-contract-note charges, allocated across fills by notional.
//! These are calculated charges, not exchange-reported per-fill commissions.
use super::super::broker_events::BrokerTrade;
use super::broker::Snapshot;
use crate::http::authenticated::ReadClient;
use anyhow::{Result, anyhow, ensure};
use nautilus_model::types::{Currency, Money};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::Deserialize;
use std::{collections::BTreeMap, str::FromStr};
pub(crate) type Fees = BTreeMap<(String, String), Money>;
#[derive(Deserialize)]
struct Charges {
    total: Decimal,
}
#[derive(Deserialize)]
struct ChargeResponse {
    exchange: String,
    tradingsymbol: String,
    transaction_type: String,
    variety: String,
    product: String,
    order_type: String,
    quantity: u32,
    price: Decimal,
    charges: Charges,
}
#[cfg(test)]
pub(crate) fn groups<'a>(
    snapshot: &'a Snapshot,
    product: &str,
    token: u32,
) -> Result<BTreeMap<String, Vec<&'a BrokerTrade>>> {
    groups_for(snapshot, product, token, "CRUDEOIL26SEPFUT")
}

pub(crate) fn groups_for<'a>(
    snapshot: &'a Snapshot,
    product: &str,
    token: u32,
    symbol: &str,
) -> Result<BTreeMap<String, Vec<&'a BrokerTrade>>> {
    let mut groups: BTreeMap<String, Vec<&BrokerTrade>> = BTreeMap::new();
    for t in snapshot
        .trades
        .iter()
        .filter(|t| t.exchange == "MCX" && t.tradingsymbol == symbol && t.product == product)
    {
        ensure!(
            t.instrument_token == token
                && t.quantity > 0
                && t.average_price > Decimal::ZERO
                && t.average_price.fract().is_zero(),
            "Invalid crude oil trade terms"
        );
        super::super::request::broker_id(&t.order_id)?;
        super::super::request::broker_id(&t.trade_id)?;
        let rows = groups.entry(t.order_id.clone()).or_default();
        ensure!(
            !rows.iter().any(|r| r.trade_id == t.trade_id),
            "Duplicate broker trade"
        );
        rows.push(t);
    }
    for (id, trades) in &groups {
        let orders: Vec<_> = snapshot
            .orders
            .iter()
            .filter(|o| &o.order_id == id)
            .collect();
        ensure!(orders.len() == 1, "Trade has no unique broker order");
        let o = orders[0];
        ensure!(
            o.exchange == "MCX"
                && o.tradingsymbol == symbol
                && o.instrument_token == token
                && o.product == product
                && o.variety == "regular"
                && matches!(o.order_type.as_str(), "LIMIT" | "MARKET")
                && o.validity == "DAY",
            "Trade/order scope mismatch"
        );
        let mut qty = 0_u32;
        for t in trades {
            ensure!(
                t.transaction_type == o.transaction_type,
                "Trade/order side mismatch"
            );
            qty = qty
                .checked_add(t.quantity)
                .ok_or_else(|| anyhow!("Trade quantity overflow"))?;
        }
        ensure!(
            qty == o.filled_quantity && qty <= o.quantity,
            "Order/trade snapshot inconsistent"
        );
        ensure!(
            matches!(o.status.as_str(), "OPEN" | "COMPLETE" | "CANCELLED"),
            "Unresolved broker order with trades"
        );
    }
    Ok(groups)
}
fn notional(t: &BrokerTrade) -> Result<Decimal> {
    t.average_price
        .checked_mul(Decimal::from(t.quantity))
        .ok_or_else(|| anyhow!("Trade notional overflow"))
}
fn total_notional(trades: &[&BrokerTrade]) -> Result<Decimal> {
    trades.iter().try_fold(Decimal::ZERO, |n, t| {
        n.checked_add(notional(t)?)
            .ok_or_else(|| anyhow!("Order notional overflow"))
    })
}
pub(crate) fn average(trades: &[&BrokerTrade]) -> Result<Decimal> {
    let quantity = trades.iter().try_fold(0_u32, |n, t| {
        n.checked_add(t.quantity)
            .ok_or_else(|| anyhow!("Trade quantity overflow"))
    })?;
    ensure!(quantity > 0, "Empty trade quantity");
    Ok(total_notional(trades)? / Decimal::from(quantity))
}
pub(crate) fn allocate(total: Decimal, trades: &[&BrokerTrade]) -> Result<Fees> {
    ensure!(
        total >= Decimal::ZERO && !trades.is_empty(),
        "Invalid calculated charges"
    );
    let cents = total
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
        .checked_mul(Decimal::from(100))
        .and_then(|v| v.to_i64())
        .ok_or_else(|| anyhow!("Calculated charges out of range"))?;
    let notional_total = total_notional(trades)?;
    ensure!(notional_total > Decimal::ZERO, "Empty trade notional");
    let mut sorted = trades.to_vec();
    sorted.sort_by(|a, b| (&a.fill_timestamp, &a.trade_id).cmp(&(&b.fill_timestamp, &b.trade_id)));
    let mut remaining = cents;
    let mut result = Fees::new();
    for (i, t) in sorted.iter().enumerate() {
        let amount = if i + 1 == sorted.len() {
            remaining
        } else {
            (Decimal::from(cents) * (notional(t)? / notional_total))
                .floor()
                .to_i64()
                .ok_or_else(|| anyhow!("Fee allocation out of range"))?
        };
        ensure!(amount >= 0 && amount <= remaining, "Invalid fee allocation");
        remaining -= amount;
        result.insert(
            (t.order_id.clone(), t.trade_id.clone()),
            Money::from_decimal(Decimal::new(amount, 2), Currency::INR())?,
        );
    }
    Ok(result)
}
pub(crate) async fn calculate(
    read: &ReadClient,
    snapshot: &Snapshot,
    product: &str,
    token: u32,
    symbol: &str,
) -> Result<Fees> {
    let groups = groups_for(snapshot, product, token, symbol)?;
    let mut result = Fees::new();
    for (id, trades) in groups {
        let o = snapshot
            .orders
            .iter()
            .find(|o| o.order_id == id)
            .expect("validated order");
        let avg = average(&trades)?;
        let number = serde_json::Number::from_str(&avg.to_string())
            .map_err(|_| anyhow!("Invalid charge calculation price"))?;
        let payload = serde_json::json!([{"order_id":id,"exchange":"MCX","tradingsymbol":o.tradingsymbol,"transaction_type":o.transaction_type,"variety":"regular","product":product,"order_type":o.order_type,"quantity":o.filled_quantity,"average_price":number}]);
        let response: Vec<ChargeResponse> = read.charges(serde_json::to_vec(&payload)?).await?;
        ensure!(
            response.len() == 1,
            "Unexpected charge calculation response"
        );
        let r = &response[0];
        ensure!(
            r.exchange == "MCX"
                && r.tradingsymbol == o.tradingsymbol
                && r.transaction_type == o.transaction_type
                && r.variety == "regular"
                && r.product == product
                && r.order_type == o.order_type
                && r.quantity == o.filled_quantity
                && r.price.round_dp(8) == avg.round_dp(8),
            "Charge calculation identity mismatch"
        );
        result.extend(allocate(r.charges.total, &trades)?);
    }
    Ok(result)
}

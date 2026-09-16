//! Commodity account balances represent trading resources, not portfolio equity.
use super::super::broker_events::{BrokerOrder, timestamp};
use super::broker::Snapshot;
use anyhow::{Result, anyhow, bail, ensure};
use nautilus_common::factories::OrderEventFactory;
use nautilus_core::{Params, UnixNanos};
use nautilus_model::{enums::*, events::AccountState, identifiers::*, reports::*, types::*};
use rust_decimal::Decimal;
use std::str::FromStr;
pub fn account(
    snapshot: &Snapshot,
    factory: &OrderEventFactory,
    now: UnixNanos,
) -> Result<AccountState> {
    let funds = &snapshot.funds;
    ensure!(
        funds.enabled && funds.utilised.debits >= Decimal::ZERO,
        "Unsupported commodity margin state"
    );
    let money = |v: Decimal| {
        Money::from_str(&format!("{:.2} INR", v.round_dp(2))).map_err(anyhow::Error::msg)
    };
    let free = money(funds.net)?;
    let locked = money(funds.utilised.debits)?;
    let total = free
        .checked_add(locked)
        .ok_or_else(|| anyhow!("Kite trading balance overflow"))?;
    let balance = AccountBalance::new_checked(total, locked, free)?;
    let mut info = Params::default();
    info.insert(
        "balance_basis".into(),
        serde_json::json!("Kite commodity trading resources; not equity"),
    );
    info.insert(
        "reported_commissions_available".into(),
        serde_json::json!(false),
    );
    Ok(factory.generate_account_state(vec![balance], vec![], true, now, now, Some(info)))
}
pub fn order(
    b: &BrokerOrder,
    account: AccountId,
    client: Option<ClientOrderId>,
    now: UnixNanos,
) -> Result<OrderStatusReport> {
    super::super::request::broker_id(&b.order_id)?;
    ensure!(
        b.variety == "regular"
            && matches!(b.order_type.as_str(), "LIMIT" | "MARKET")
            && b.validity == "DAY",
        "Unsupported broker order report type"
    );
    ensure!(
        b.quantity > 0 && b.filled_quantity <= b.quantity,
        "Invalid broker order report quantities"
    );
    let status = match b.status.as_str() {
        "OPEN" if b.filled_quantity == 0 => OrderStatus::Accepted,
        "OPEN" if b.filled_quantity < b.quantity => OrderStatus::PartiallyFilled,
        "COMPLETE" if b.filled_quantity == b.quantity => OrderStatus::Filled,
        "CANCELLED" => OrderStatus::Canceled,
        "REJECTED" if b.filled_quantity == 0 => OrderStatus::Rejected,
        "PUT ORDER REQ RECEIVED" | "VALIDATION PENDING" | "OPEN PENDING"
            if b.filled_quantity == 0 =>
        {
            OrderStatus::Submitted
        }
        _ => bail!("Unresolved broker order status"),
    };
    let side = match b.transaction_type.as_str() {
        "BUY" => OrderSide::Buy,
        "SELL" => OrderSide::Sell,
        _ => bail!("Invalid broker side"),
    };
    let accepted = b
        .exchange_timestamp
        .as_deref()
        .map(timestamp)
        .transpose()?
        .unwrap_or(UnixNanos::from(0));
    ensure!(
        !matches!(
            status,
            OrderStatus::Accepted | OrderStatus::PartiallyFilled | OrderStatus::Filled
        ) || accepted.as_u64() > 0,
        "Exchange acceptance timestamp missing"
    );
    let last = timestamp(
        b.exchange_update_timestamp
            .as_deref()
            .unwrap_or(&b.order_timestamp),
    )?;
    ensure!(
        accepted <= last && last <= now,
        "Invalid broker report chronology"
    );
    let protected = b
        .market_protection
        .is_some_and(|p| p == Decimal::from(-1) || (p > Decimal::ZERO && p <= Decimal::from(100)));
    ensure!(
        b.order_type != "MARKET" || protected,
        "Unprotected market report"
    );
    ensure!(
        protected || b.price > Decimal::ZERO && b.price.fract().is_zero(),
        "Invalid crude oil limit price"
    );
    let mut report = OrderStatusReport::new(
        account,
        "CRUDEOIL26SEPFUT.MCX".into(),
        client,
        VenueOrderId::from(b.order_id.as_str()),
        Some(side),
        if protected {
            OrderType::Market
        } else {
            OrderType::Limit
        },
        TimeInForce::Day,
        status,
        Quantity::from(b.quantity),
        Quantity::from(b.filled_quantity),
        accepted,
        last,
        now,
        None,
    );
    if !protected {
        report.price =
            Some(Price::from_str(&b.price.normalize().to_string()).map_err(anyhow::Error::msg)?);
    }
    Ok(report)
}
pub fn positions(
    snapshot: &Snapshot,
    account: AccountId,
    product: &str,
    token: u32,
    now: UnixNanos,
) -> Result<Vec<PositionStatusReport>> {
    let rows: Vec<_> = snapshot
        .positions
        .iter()
        .filter(|p| p.exchange == "MCX" && p.tradingsymbol == "CRUDEOIL26SEPFUT")
        .collect();
    ensure!(
        rows.iter()
            .all(|p| p.instrument_token == token && (p.product == product || p.quantity == 0)),
        "Native netting cannot merge another product or instrument token"
    );
    let selected: Vec<_> = rows.iter().filter(|p| p.product == product).collect();
    ensure!(selected.len() <= 1, "Duplicate broker position");
    let (qty, avg) = if let Some(p) = selected.first() {
        (
            p.quantity,
            if p.quantity == 0 {
                None
            } else {
                ensure!(p.average_price > Decimal::ZERO, "Invalid position cost");
                Some(p.average_price)
            },
        )
    } else {
        (0, None)
    };
    let abs = qty
        .checked_abs()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| anyhow!("Position quantity out of range"))?;
    Ok(vec![PositionStatusReport::new(
        account,
        "CRUDEOIL26SEPFUT.MCX".into(),
        if qty > 0 {
            PositionSide::Long
        } else if qty < 0 {
            PositionSide::Short
        } else {
            PositionSide::Flat
        },
        Quantity::from(abs),
        now,
        now,
        None,
        None,
        avg,
    )])
}

pub(crate) fn fills(
    snapshot: &Snapshot,
    account: AccountId,
    product: &str,
    token: u32,
    now: UnixNanos,
    fees: &super::fees::Fees,
    owners: &std::collections::BTreeMap<String, ClientOrderId>,
) -> Result<Vec<FillReport>> {
    let groups = super::fees::groups(snapshot, product, token)?;
    let mut reports = vec![];
    for trades in groups.values() {
        for t in trades {
            let order = snapshot
                .orders
                .iter()
                .find(|o| o.order_id == t.order_id)
                .expect("validated trade order");
            let ts = timestamp(&t.fill_timestamp)?;
            let accepted = timestamp(
                order
                    .exchange_timestamp
                    .as_deref()
                    .ok_or_else(|| anyhow!("Fill acceptance timestamp missing"))?,
            )?;
            let last = timestamp(
                order
                    .exchange_update_timestamp
                    .as_deref()
                    .unwrap_or(&order.order_timestamp),
            )?;
            ensure!(
                accepted <= ts && ts <= last && last <= now,
                "Inconsistent fill chronology"
            );
            let fee = fees
                .get(&(t.order_id.clone(), t.trade_id.clone()))
                .ok_or_else(|| anyhow!("Calculated fill commission missing"))?;
            let side = match t.transaction_type.as_str() {
                "BUY" => OrderSide::Buy,
                "SELL" => OrderSide::Sell,
                _ => bail!("Invalid fill side"),
            };
            reports.push(FillReport::new(
                account,
                "CRUDEOIL26SEPFUT.MCX".into(),
                t.order_id.as_str().into(),
                t.trade_id.as_str().into(),
                side,
                Quantity::from(t.quantity),
                Price::from_str(&t.average_price.normalize().to_string())
                    .map_err(anyhow::Error::msg)?,
                *fee,
                LiquiditySide::NoLiquiditySide,
                owners.get(&t.order_id).copied(),
                None,
                ts,
                now,
                None,
            ));
        }
    }
    reports.sort_by_key(|r| (r.ts_event, r.venue_order_id, r.trade_id));
    Ok(reports)
}

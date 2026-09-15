use super::snapshot::Snapshot;
use crate::account::products::Product;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize)]
pub struct Summary {
    pub event: &'static str,
    pub instrument_id: String,
    pub trading_date_ist: String,
    pub stable_observation: bool,
    pub consistency_checks_passed: bool,
    pub target_orders: usize,
    pub target_trades: usize,
    pub target_position_buckets: usize,
    pub external_orders: usize,
    pub nonterminal_orders: usize,
    pub nonzero_position_buckets: usize,
    pub profile_mis_enabled: bool,
    pub profile_nrml_enabled: bool,
    pub issues: BTreeSet<&'static str>,
    pub live_orders_enabled: bool,
    pub execution_ready: bool,
}
pub(super) fn check(s: &Snapshot, symbol: &str, token: u32, products: &[String]) -> Summary {
    let mut issues = BTreeSet::new();
    let mut order_ids = BTreeSet::new();
    let mut trade_ids = BTreeSet::new();
    let mut fills: BTreeMap<&str, i128> = BTreeMap::new();
    let mut activity: BTreeMap<&str, (i128, i128)> = BTreeMap::new();
    for o in &s.orders {
        if o.order_id.is_empty() || !order_ids.insert(o.order_id.as_str()) {
            issues.insert("invalid_or_duplicate_order_id");
        }
        if o.exchange != "MCX" || o.tradingsymbol != symbol || o.instrument_token != token {
            issues.insert("instrument_identity_mismatch");
        }
        if Product::parse(&o.product).is_none() || !products.contains(&o.product) {
            issues.insert("unsupported_or_disabled_product");
        }
        if !matches!(o.transaction_type.as_str(), "BUY" | "SELL") {
            issues.insert("unknown_transaction_side");
        }
        if o.quantity <= 0
            || o.filled_quantity < 0
            || o.pending_quantity < 0
            || o.cancelled_quantity < 0
            || o.filled_quantity > o.quantity
            || o.pending_quantity > o.quantity
            || o.cancelled_quantity > o.quantity
            || i128::from(o.filled_quantity) + i128::from(o.cancelled_quantity)
                > i128::from(o.quantity)
            || (!o.terminal()
                && i128::from(o.filled_quantity) + i128::from(o.pending_quantity)
                    > i128::from(o.quantity))
            || (o.status == "COMPLETE" && o.filled_quantity != o.quantity)
        {
            issues.insert("invalid_order_quantities");
        }
        if !matches!(
            o.status.as_str(),
            "COMPLETE"
                | "CANCELLED"
                | "REJECTED"
                | "OPEN"
                | "TRIGGER PENDING"
                | "VALIDATION PENDING"
                | "PUT ORDER REQ RECEIVED"
                | "OPEN PENDING"
                | "MODIFY VALIDATION PENDING"
                | "MODIFY PENDING"
                | "CANCEL PENDING"
                | "AMO REQ RECEIVED"
        ) {
            issues.insert("unknown_order_status");
        }
    }
    for t in &s.trades {
        if t.trade_id.is_empty()
            || !trade_ids.insert((
                t.exchange.as_str(),
                t.trade_id.as_str(),
                t.order_id.as_str(),
            ))
        {
            issues.insert("invalid_or_duplicate_trade_id");
        }
        if t.exchange != "MCX" || t.tradingsymbol != symbol || t.instrument_token != token {
            issues.insert("instrument_identity_mismatch");
        }
        if t.quantity <= 0 {
            issues.insert("invalid_trade_quantity");
        }
        if Product::parse(&t.product).is_none() || !products.contains(&t.product) {
            issues.insert("unsupported_or_disabled_product");
        }
        match s.orders.iter().find(|o| o.order_id == t.order_id) {
            Some(o) if o.product == t.product && o.transaction_type == t.transaction_type => {}
            _ => {
                issues.insert("trade_order_mismatch");
            }
        }
        *fills.entry(&t.order_id).or_default() += i128::from(t.quantity);
        let totals = activity.entry(&t.product).or_default();
        match t.transaction_type.as_str() {
            "BUY" => totals.0 += i128::from(t.quantity),
            "SELL" => totals.1 += i128::from(t.quantity),
            _ => {
                issues.insert("unknown_transaction_side");
            }
        }
    }
    for o in &s.orders {
        if fills.get(o.order_id.as_str()).copied().unwrap_or(0) != i128::from(o.filled_quantity) {
            issues.insert("order_fill_quantity_mismatch");
        }
    }
    for rows in [&s.positions.net, &s.positions.day] {
        let mut buckets = BTreeSet::new();
        for p in rows {
            if !buckets.insert(p.product.as_str()) {
                issues.insert("duplicate_position_bucket");
            }
            if p.exchange != "MCX" || p.tradingsymbol != symbol || p.instrument_token != token {
                issues.insert("instrument_identity_mismatch");
            }
            if Product::parse(&p.product).is_none() || !products.contains(&p.product) {
                issues.insert("unsupported_or_disabled_product");
            }
            if p.day_buy_quantity < 0 || p.day_sell_quantity < 0 {
                issues.insert("invalid_position_activity");
            }
        }
    }
    // Net positions include carry-in. Do not mistake today's trades for the net holding.
    for p in &s.positions.net {
        if i128::from(p.quantity)
            != i128::from(p.overnight_quantity) + i128::from(p.day_buy_quantity)
                - i128::from(p.day_sell_quantity)
        {
            issues.insert("position_carry_activity_mismatch");
        }
        if activity
            .get(p.product.as_str())
            .copied()
            .unwrap_or_default()
            != (
                i128::from(p.day_buy_quantity),
                i128::from(p.day_sell_quantity),
            )
        {
            issues.insert("position_trade_activity_mismatch");
        }
    }
    for product in activity.keys() {
        if !s.positions.net.iter().any(|p| p.product == *product) {
            issues.insert("trade_position_bucket_missing");
        }
    }
    Summary {
        event: "reconciliation_complete",
        instrument_id: format!("{symbol}.MCX"),
        trading_date_ist: String::new(),
        stable_observation: false,
        consistency_checks_passed: issues.is_empty(),
        target_orders: s.orders.len(),
        target_trades: s.trades.len(),
        target_position_buckets: s.positions.net.len(),
        external_orders: s.orders.len(),
        nonterminal_orders: s.orders.iter().filter(|o| !o.terminal()).count(),
        nonzero_position_buckets: s.positions.net.iter().filter(|p| p.quantity != 0).count(),
        profile_mis_enabled: products.iter().any(|p| p == "MIS"),
        profile_nrml_enabled: products.iter().any(|p| p == "NRML"),
        issues,
        live_orders_enabled: false,
        execution_ready: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        orders::Order,
        positions::{Position, Positions},
        trades::Trade,
    };
    use serde_json::json;
    fn fixture() -> Snapshot {
        let order: Order = serde_json::from_value(json!({
            "order_id":"private-order", "exchange":"MCX", "tradingsymbol":"CRUDEOIL26SEPFUT", "instrument_token":144870151,
            "product":"NRML", "transaction_type":"BUY", "status":"COMPLETE", "quantity":2,
            "filled_quantity":2, "pending_quantity":0, "cancelled_quantity":0
        })).unwrap();
        let trade: Trade = serde_json::from_value(json!({
            "order_id":"private-order", "trade_id":"private-trade", "exchange":"MCX", "tradingsymbol":"CRUDEOIL26SEPFUT",
            "instrument_token":144870151, "product":"NRML", "transaction_type":"BUY", "quantity":2
        })).unwrap();
        let position: Position = serde_json::from_value(json!({
            "exchange":"MCX", "tradingsymbol":"CRUDEOIL26SEPFUT", "instrument_token":144870151, "product":"NRML",
            "quantity":5, "overnight_quantity":3, "day_buy_quantity":2, "day_sell_quantity":0
        })).unwrap();
        Snapshot {
            orders: vec![order],
            trades: vec![trade],
            positions: Positions {
                net: vec![position],
                day: vec![],
            },
        }
    }
    fn run(s: &Snapshot) -> Summary {
        check(
            s,
            "CRUDEOIL26SEPFUT",
            144870151,
            &["NRML".into(), "MIS".into()],
        )
    }
    #[test]
    fn reconciles_carry_and_today_fills_without_claiming_execution_readiness() {
        let result = run(&fixture());
        assert!(result.consistency_checks_passed);
        assert!(!result.execution_ready);
        assert!(!result.live_orders_enabled);
        assert_eq!(result.external_orders, 1);
        let output = serde_json::to_string(&result).unwrap();
        assert!(!output.contains("private-order"));
        assert!(!output.contains("private-trade"));
        assert!(!output.contains("overnight_quantity"));
    }
    #[test]
    fn rejects_duplicate_fills_and_missing_order() {
        let mut s = fixture();
        s.trades.push(s.trades[0].clone());
        assert!(run(&s).issues.contains("invalid_or_duplicate_trade_id"));
        assert!(run(&s).issues.contains("order_fill_quantity_mismatch"));
        s.orders.clear();
        assert!(run(&s).issues.contains("trade_order_mismatch"));
    }
    #[test]
    fn separates_mis_and_nrml_and_flags_possible_conversion() {
        let mut s = fixture();
        s.positions.net[0].product = "MIS".into();
        let result = run(&s);
        assert!(result.issues.contains("position_trade_activity_mismatch"));
        assert!(result.issues.contains("trade_position_bucket_missing"));
    }
    #[test]
    fn rejects_unknown_product_identity_status_and_side() {
        let mut s = fixture();
        s.orders[0].product = "CNC".into();
        s.orders[0].instrument_token = 7;
        s.orders[0].status = "private-unknown".into();
        s.orders[0].transaction_type = "private-side".into();
        let result = run(&s);
        for issue in [
            "unsupported_or_disabled_product",
            "instrument_identity_mismatch",
            "unknown_order_status",
            "unknown_transaction_side",
        ] {
            assert!(result.issues.contains(issue));
        }
        assert!(!serde_json::to_string(&result).unwrap().contains("private-"));
    }
    #[test]
    fn detects_position_and_order_quantity_errors_without_overflow() {
        let mut s = fixture();
        s.positions.net[0].overnight_quantity = i64::MAX;
        s.positions.net[0].day_buy_quantity = i64::MAX;
        s.orders[0].pending_quantity = i64::MAX;
        assert!(run(&s).issues.contains("position_carry_activity_mismatch"));
        assert!(run(&s).issues.contains("invalid_order_quantities"));
        s.trades[0].quantity = -1;
        assert!(run(&s).issues.contains("invalid_trade_quantity"));
    }
    #[test]
    fn empty_account_is_consistent_but_not_execution_ready() {
        let s = Snapshot {
            orders: vec![],
            trades: vec![],
            positions: Positions {
                net: vec![],
                day: vec![],
            },
        };
        let result = run(&s);
        assert!(result.consistency_checks_passed);
        assert!(!result.execution_ready);
    }
    #[test]
    fn partial_cancelled_fill_is_matched_to_trade_quantity() {
        let mut s = fixture();
        s.orders[0].quantity = 3;
        s.orders[0].cancelled_quantity = 1;
        // Kite may retain pending_quantity on a cancelled order.
        s.orders[0].pending_quantity = 1;
        s.orders[0].status = "CANCELLED".into();
        assert!(run(&s).consistency_checks_passed);
        s.orders[0].filled_quantity = 1;
        assert!(run(&s).issues.contains("order_fill_quantity_mismatch"));
    }
}

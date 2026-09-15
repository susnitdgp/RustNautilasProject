//! Offline review only: no execution client, no broker connection, no resubmission.
use super::ledger::Record;
use anyhow::{Result, anyhow, ensure};
use nautilus_model::{
    enums::OrderSide,
    orders::{Order, OrderAny},
};
use std::collections::{BTreeMap, BTreeSet};
pub fn review(namespace: &str) -> Result<serde_json::Value> {
    review_at(&kite_journal::connection::url_from_env()?, namespace)
}
pub fn review_at(url: &str, namespace: &str) -> Result<serde_json::Value> {
    ensure!(
        !namespace.is_empty()
            && namespace.len() <= 64
            && namespace
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "Invalid recovery namespace"
    );
    let mut c = kite_journal::connection::connect(url)?;
    let key = format!("susanta:nautilus:native-kite:commands:{{{namespace}}}");
    let count: usize = redis::cmd("HLEN")
        .arg(&key)
        .query(&mut c)
        .map_err(|_| anyhow!("Native journal unavailable"))?;
    ensure!(
        (1..=20001).contains(&count),
        "Native journal missing or oversized"
    );
    let values: BTreeMap<String, String> = redis::cmd("HGETALL")
        .arg(&key)
        .query(&mut c)
        .map_err(|_| anyhow!("Native journal unavailable"))?;
    ensure!(
        values.get("scope").map(String::as_str) == Some("NATIVE_KITE_DISABLED_V1"),
        "Invalid journal scope"
    );
    let mut orders = Vec::new();
    let mut tags = BTreeSet::new();
    let mut brokers = BTreeSet::new();
    let mut exposure = rust_decimal::Decimal::ZERO;
    for (field, value) in values.iter().filter(|(f, _)| f.starts_with("order:")) {
        let r: Record =
            serde_json::from_str(value).map_err(|_| anyhow!("Invalid native journal record"))?;
        ensure!(r.events.len() <= 10000, "Oversized native event history");
        let o =
            OrderAny::from_events(r.events).map_err(|_| anyhow!("Invalid native event history"))?;
        ensure!(
            field == &format!("order:{}", o.client_order_id())
                && tags.insert(r.tag.clone())
                && values.get(&format!("tag:{}", r.tag)) == Some(field),
            "Native journal ownership mismatch"
        );
        if let Some(b) = &r.broker_id {
            ensure!(brokers.insert(b.clone()), "Duplicate broker ownership");
        }
        exposure += if o.order_side() == OrderSide::Buy {
            o.filled_qty().as_decimal()
        } else {
            -o.filled_qty().as_decimal()
        };
        orders.push(serde_json::json!({"client_order_id":o.client_order_id(),"broker_order_id":r.broker_id,"status":o.status(),"closed":o.is_closed(),"outcome":r.outcome,"management":r.management,"filled_contracts":o.filled_qty().to_string()}));
    }
    ensure!(
        count == 1 + orders.len() * 2,
        "Unrecognized native journal fields"
    );
    let unresolved = orders.iter().filter(|o| o["closed"] != true).count();
    Ok(
        serde_json::json!({"event":"native_kite_recovery_review","namespace":namespace,"orders":orders,"unresolved":unresolved,"journal_exposure":exposure.to_string(),"requires_review":unresolved>0||!exposure.is_zero(),"broker_reconciliation_required":true,"resubmissions":0,"automatic_resume_enabled":false,"live_orders_enabled":false}),
    )
}

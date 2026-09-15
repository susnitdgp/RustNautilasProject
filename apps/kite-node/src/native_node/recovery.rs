//! Read-only native Redis reconstruction. Never registers an execution client.
use anyhow::{Result, ensure};
use nautilus_common::cache::database::CacheDatabaseFactory;
use nautilus_core::UUID4;
use nautilus_model::orders::Order;
use std::str::FromStr;
pub fn run(namespace: &str) -> Result<()> {
    let instance = UUID4::from_str(namespace).map_err(anyhow::Error::msg)?;
    let mut adapter = nautilus_common::live::get_runtime().block_on(
        super::redis_cache::Factory(super::persistence::redis_config()?).create(
            "SUSANTA-001".into(),
            instance,
            super::persistence::cache_config(),
        ),
    )?;
    let map = nautilus_common::live::get_runtime().block_on(adapter.load_all())?;
    ensure!(!map.instruments.is_empty(), "Native Redis run not found");
    let open_orders = map.orders.values().filter(|o| !o.is_closed()).count();
    let net: f64 = map.positions.values().map(|p| p.signed_qty).sum();
    println!(
        "{}",
        serde_json::json!({"event":"native_redis_recovery_complete","namespace":namespace,
  "instruments":map.instruments.len(),"accounts":map.accounts.len(),"orders":map.orders.len(),
  "positions":map.positions.len(),"open_orders":open_orders,"open_contracts":net,
  "requires_review":open_orders>0||net!=0.0,"resubmissions":0,"automatic_resume_enabled":false,
  "live_orders_enabled":false})
    );
    adapter.close()?;
    Ok(())
}

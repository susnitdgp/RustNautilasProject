use crate::{native_paper_command as native, runtime::core::Core};
use anyhow::{Result, ensure};
use kite_journal::{model::Side, state::Status, store::Journal};
use kite_paper::{control_store::ControlStore, outbox::Outbox};
use nautilus_execution::engine::ExecutionEngine;
use nautilus_model::{
    enums::{OmsType, OrderStatus},
    identifiers::{ClientOrderId, StrategyId},
    orders::Order,
};
pub fn run(namespace: &str) -> Result<()> {
    let url = kite_journal::connection::url_from_env()?;
    println!(
        "{}",
        serde_json::json!({"namespace":namespace,"result":inspect(&url,namespace)?})
    );
    Ok(())
}
pub fn inspect(url: &str, namespace: &str) -> Result<serde_json::Value> {
    let control = ControlStore::read_at(url, namespace)?;
    ensure!(
        control["version"] == 1,
        "Missing native paper session checkpoint; review required"
    );
    let journal = Journal::open_at(url, namespace)?;
    let events = Outbox::read_at(url, namespace)?;
    let (instrument, ts) = super::simulation::fixture()?;
    let core = Core::new(&instrument);
    core.cache.borrow_mut().add_account(native::account(ts))?;
    let mut engine = ExecutionEngine::new(core.clock.clone(), core.cache.clone(), None);
    engine.register_oms_type(StrategyId::from("CROSSOVER-001"), OmsType::Netting);
    native::apply(&core, &mut engine, events)?;
    let mut net = 0i64;
    let mut unresolved = 0usize;
    for o in journal.state().orders() {
        net += if matches!(o.intent.side, Side::Buy) {
            i64::from(o.filled)
        } else {
            -i64::from(o.filled)
        };
        let id = ClientOrderId::from(o.intent.id.as_str());
        let cache = core.cache.borrow();
        let native = cache
            .order(&id)
            .ok_or_else(|| anyhow::anyhow!("Journal/outbox mismatch; review required"))?;
        ensure!(
            native.filled_qty() == nautilus_model::types::Quantity::from(o.filled),
            "Recovered fill mismatch"
        );
        match o.status {
            Status::Filled => ensure!(
                native.status() == OrderStatus::Filled,
                "Recovered status mismatch"
            ),
            Status::Cancelled => ensure!(
                native.status() == OrderStatus::Canceled,
                "Recovered status mismatch"
            ),
            _ => {
                unresolved += 1;
            }
        }
    }
    let native_net: f64 = core
        .cache
        .borrow()
        .positions_open(None, None, None, None, None)
        .iter()
        .map(|p| p.signed_qty)
        .sum();
    ensure!(
        native_net == net as f64,
        "Recovered native position mismatch"
    );
    let finished = control["finished"] == true;
    if finished {
        ensure!(
            control["open_contracts"].as_i64() == Some(net),
            "Finished checkpoint mismatch"
        );
    }
    Ok(
        serde_json::json!({"event":"paper_recovery_inspected","native_replay_verified":true,"open_contracts":net,"unresolved_orders":unresolved,"session_finished":finished,"requires_review":!finished || unresolved>0 || net!=0,"resubmissions":0,"automatic_resume_enabled":false,"live_orders_enabled":false}),
    )
}

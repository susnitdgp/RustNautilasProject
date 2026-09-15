use super::{Decision, Limiter, policy::Policy};
use crate::{
    coordinator,
    mock::{MockBroker, Outcome},
};
use anyhow::{Result, ensure};
use kite_journal::{
    connection,
    model::{Event, Intent, Product, Side},
    state::Status,
    store::Journal,
};
use serde::Serialize;
#[derive(Serialize)]
pub struct Summary {
    pub event: &'static str,
    pub workers: usize,
    pub admitted: usize,
    pub deferred: usize,
    pub restart_preserves_budget: bool,
    pub cooldown_shared: bool,
    pub deferred_intent_stays_prepared: bool,
    pub blocked_mock_calls: usize,
    pub broker_accessed: bool,
    pub live_orders_enabled: bool,
}
pub fn run(namespace: &str) -> Result<Summary> {
    run_at(&connection::url_from_env()?, namespace)
}
pub fn run_at(url: &str, namespace: &str) -> Result<Summary> {
    let policy = Policy {
        per_second: 1,
        per_minute: 1,
        per_day: 1,
    };
    let mut first = Limiter::create_at(url, namespace, policy)?;
    let mut second = Limiter::open_at(url, namespace, policy)?;
    let a = first.reserve()?;
    let b = second.reserve()?;
    drop(first);
    drop(second);
    let mut reopened = Limiter::open_at(url, namespace, policy)?;
    let restart_preserves_budget = matches!(reopened.reserve()?, Decision::Deferred { .. });
    reopened.cooldown(86400000)?;
    let mut worker = Limiter::open_at(url, namespace, policy)?;
    let cooldown_shared =
        matches!(worker.reserve()?,Decision::Deferred {retry_after_ms} if retry_after_ms>86300000);
    let mut journal = Journal::create_at(url, namespace)?;
    journal.append(Event::Intent {
        intent: Intent {
            id: "Blocked".into(),
            symbol: "CRUDEOIL26SEPFUT".into(),
            side: Side::Buy,
            product: Product::Nrml,
            quantity: 1,
            limit_price_paise: 600000,
        },
    })?;
    let mut broker = MockBroker::new(Outcome::Accepted("Never".into()));
    let blocked = coordinator::submit(&mut journal, &mut broker, "Blocked", &mut worker).is_err();
    let deferred_intent_stays_prepared =
        journal.state().order("Blocked")?.status == Status::Prepared;
    ensure!(
        a == Decision::Allowed
            && matches!(b, Decision::Deferred { .. })
            && restart_preserves_budget
            && cooldown_shared
            && blocked
            && deferred_intent_stays_prepared
            && broker.calls == 0,
        "Rate-limit verification failed"
    );
    Ok(Summary {
        event: "rate_limit_simulation_complete",
        workers: 2,
        admitted: 1,
        deferred: 1,
        restart_preserves_budget,
        cooldown_shared,
        deferred_intent_stays_prepared,
        blocked_mock_calls: broker.calls,
        broker_accessed: false,
        live_orders_enabled: false,
    })
}

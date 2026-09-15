use crate::{
    coordinator,
    mock::{MockBroker, Outcome},
};
use anyhow::{Result, ensure};
use kite_journal::{
    model::{Event, Intent, Product, Side},
    state::Status,
    store::Journal,
};
use serde::Serialize;

#[derive(Serialize)]
pub struct Summary {
    pub event: &'static str,
    pub simulated_submissions: usize,
    pub filled_contracts: u32,
    pub duplicate_fill_ignored: bool,
    pub ambiguous_retry_blocked_after_restart: bool,
    pub interrupted_retry_blocked_after_restart: bool,
    pub journal_records: usize,
    pub broker_accessed: bool,
    pub live_orders_enabled: bool,
}
fn intent(id: &str) -> Event {
    Event::Intent {
        intent: Intent {
            id: id.into(),
            symbol: "CRUDEOIL26SEPFUT".into(),
            side: Side::Buy,
            product: Product::Nrml,
            quantity: 2,
            limit_price_paise: 600000,
        },
    }
}
pub fn run(namespace: &str) -> Result<Summary> {
    run_at(&kite_journal::connection::url_from_env()?, namespace)
}
pub fn run_at(url: &str, namespace: &str) -> Result<Summary> {
    let mut limiter = crate::rate_limit::Limiter::create_at(
        url,
        namespace,
        crate::rate_limit::policy::Policy::default(),
    )?;
    let mut journal = Journal::create_at(url, namespace)?;
    journal.append(intent("MockFilled"))?;
    let mut accepted = MockBroker::new(Outcome::Accepted("MockBroker1".into()));
    coordinator::submit(&mut journal, &mut accepted, "MockFilled", &mut limiter)?;
    let fill = Event::Fill {
        id: "MockFilled".into(),
        broker_id: "MockBroker1".into(),
        trade_id: "Trade1".into(),
        quantity: 1,
        price_paise: 600000,
    };
    journal.append(fill.clone())?;
    let duplicate_fill_ignored = !journal.append(fill)?;
    journal.append(Event::Fill {
        id: "MockFilled".into(),
        broker_id: "MockBroker1".into(),
        trade_id: "Trade2".into(),
        quantity: 1,
        price_paise: 599900,
    })?;
    journal.append(intent("MockTimeout"))?;
    let mut timeout = MockBroker::new(Outcome::AmbiguousTimeout);
    coordinator::submit(&mut journal, &mut timeout, "MockTimeout", &mut limiter)?;
    journal.append(intent("MockInterrupted"))?;
    journal.append(Event::Dispatch {
        id: "MockInterrupted".into(),
    })?;
    drop(journal);
    let mut journal = Journal::open_at(url, namespace)?;
    let mut retry = MockBroker::new(Outcome::Accepted("MustNotRun".into()));
    let ambiguous_retry_blocked_after_restart =
        coordinator::submit(&mut journal, &mut retry, "MockTimeout", &mut limiter).is_err();
    let interrupted_retry_blocked_after_restart =
        coordinator::submit(&mut journal, &mut retry, "MockInterrupted", &mut limiter).is_err();
    ensure!(
        retry.calls == 0
            && duplicate_fill_ignored
            && ambiguous_retry_blocked_after_restart
            && interrupted_retry_blocked_after_restart,
        "Mock execution verification failed"
    );
    ensure!(
        journal.state().order("MockFilled")?.status == Status::Filled,
        "Fill recovery failed"
    );
    Ok(Summary {
        event: "execution_simulation_complete",
        simulated_submissions: accepted.calls + timeout.calls,
        filled_contracts: journal.state().order("MockFilled")?.filled,
        duplicate_fill_ignored,
        ambiguous_retry_blocked_after_restart,
        interrupted_retry_blocked_after_restart,
        journal_records: journal.record_count(),
        broker_accessed: false,
        live_orders_enabled: false,
    })
}

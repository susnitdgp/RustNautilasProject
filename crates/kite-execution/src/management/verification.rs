use super::coordinator as management;
use crate::{
    coordinator,
    mock::{MockBroker, Outcome},
    rate_limit::{Limiter, policy::Policy},
};
use anyhow::{Result, ensure};
use kite_journal::{
    actions::{Change, CommandStatus},
    connection,
    model::{Event, Intent, Product, Side},
    state::Status,
    store::Journal,
};
use serde::Serialize;
#[derive(Serialize)]
pub struct Summary {
    pub event: &'static str,
    pub mock_calls: usize,
    pub modification_confirmed: bool,
    pub cancel_ack_kept_order_open: bool,
    pub cancellation_confirmed: bool,
    pub partial_fill_preserved: bool,
    pub ambiguous_cancel_retry_blocked_after_restart: bool,
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
    run_at(&connection::url_from_env()?, namespace)
}
pub fn run_at(url: &str, namespace: &str) -> Result<Summary> {
    let mut journal = Journal::create_at(url, namespace)?;
    let mut limiter = Limiter::create_at(url, namespace, Policy::default())?;
    journal.append(intent("Managed"))?;
    let mut broker = MockBroker::new(Outcome::Accepted("Broker1".into()));
    coordinator::submit(&mut journal, &mut broker, "Managed", &mut limiter)?;
    management::prepare(
        &mut journal,
        "Managed",
        "Modify1",
        Change::Modify {
            quantity: 3,
            limit_price_paise: 600100,
        },
    )?;
    management::send(
        &mut journal,
        &mut broker,
        "Managed",
        "Modify1",
        &mut limiter,
    )?;
    ensure!(
        journal.state().order("Managed")?.intent.quantity == 2,
        "Acknowledgement changed order terms"
    );
    management::confirm(&mut journal, "Managed", "Modify1", "Broker1")?;
    let modification_confirmed = journal.state().order("Managed")?.intent.quantity == 3;
    journal.append(Event::Fill {
        id: "Managed".into(),
        broker_id: "Broker1".into(),
        trade_id: "Trade1".into(),
        quantity: 1,
        price_paise: 600000,
    })?;
    management::prepare(&mut journal, "Managed", "Cancel1", Change::Cancel)?;
    management::send(
        &mut journal,
        &mut broker,
        "Managed",
        "Cancel1",
        &mut limiter,
    )?;
    let cancel_ack_kept_order_open =
        journal.state().order("Managed")?.status == Status::PartiallyFilled;
    management::confirm(&mut journal, "Managed", "Cancel1", "Broker1")?;
    let cancellation_confirmed = journal.state().order("Managed")?.status == Status::Cancelled;
    let partial_fill_preserved = journal.state().order("Managed")?.filled == 1;
    journal.append(intent("Uncertain"))?;
    let mut second = MockBroker::new(Outcome::Accepted("Broker2".into()));
    coordinator::submit(&mut journal, &mut second, "Uncertain", &mut limiter)?;
    management::prepare(&mut journal, "Uncertain", "Cancel2", Change::Cancel)?;
    let mut timeout = MockBroker::new(Outcome::AmbiguousTimeout);
    management::send(
        &mut journal,
        &mut timeout,
        "Uncertain",
        "Cancel2",
        &mut limiter,
    )?;
    drop(journal);
    let mut journal = Journal::open_at(url, namespace)?;
    let mut never = MockBroker::new(Outcome::Accepted("Broker2".into()));
    let ambiguous_cancel_retry_blocked_after_restart = management::send(
        &mut journal,
        &mut never,
        "Uncertain",
        "Cancel2",
        &mut limiter,
    )
    .is_err()
        && never.calls == 0
        && journal.state().order("Uncertain")?.commands.entries["Cancel2"].status
            == CommandStatus::Unknown;
    ensure!(
        modification_confirmed
            && cancel_ack_kept_order_open
            && cancellation_confirmed
            && partial_fill_preserved
            && ambiguous_cancel_retry_blocked_after_restart,
        "Management simulation failed"
    );
    Ok(Summary {
        event: "order_management_simulation_complete",
        mock_calls: broker.calls + second.calls + timeout.calls,
        modification_confirmed,
        cancel_ack_kept_order_open,
        cancellation_confirmed,
        partial_fill_preserved,
        ambiguous_cancel_retry_blocked_after_restart,
        broker_accessed: false,
        live_orders_enabled: false,
    })
}

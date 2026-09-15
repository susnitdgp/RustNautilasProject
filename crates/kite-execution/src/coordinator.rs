use crate::{
    mock::{MockBroker, Outcome},
    translation,
};
use anyhow::Result;
use kite_journal::{model::Event, store::Journal};
/// Deliberately accepts only the concrete mock broker.
pub fn submit(journal: &mut Journal, broker: &mut MockBroker, id: &str) -> Result<()> {
    let request = translation::translate(&journal.state().order(id)?.intent)?;
    journal.append(Event::Dispatch { id: id.into() })?;
    // A process exit after this point leaves Dispatching and blocks automatic resubmission.
    let response = broker.submit(&request);
    let event = match response {
        Outcome::Accepted(broker_id) => Event::Acknowledged {
            id: id.into(),
            broker_id,
        },
        Outcome::AmbiguousTimeout => Event::Unknown { id: id.into() },
        Outcome::Rejected => Event::Rejected { id: id.into() },
    };
    journal.append(event)?;
    Ok(())
}

use crate::rate_limit::{Decision, Limiter};
use crate::{
    mock::{MockBroker, Outcome},
    translation,
};
use anyhow::{Result, ensure};
use kite_journal::state::Status;
use kite_journal::{model::Event, store::Journal};
/// Deliberately accepts only the concrete mock broker.
pub fn submit(
    journal: &mut Journal,
    broker: &mut MockBroker,
    id: &str,
    limiter: &mut Limiter,
) -> Result<()> {
    let request = translation::translate(&journal.state().order(id)?.intent)?;
    ensure!(
        journal.state().order(id)?.status == Status::Prepared,
        "Intent already attempted; blind retry blocked"
    );
    ensure!(
        limiter.reserve()? == Decision::Allowed,
        "Order request deferred by shared rate budget"
    );
    journal.append(Event::Dispatch { id: id.into() })?;
    // A process exit after this point leaves Dispatching and blocks automatic resubmission.
    let response = broker.submit(&request);
    let event = match response {
        Outcome::RateLimited { retry_after_ms } => {
            limiter.cooldown(retry_after_ms)?;
            Event::Unknown { id: id.into() }
        }
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

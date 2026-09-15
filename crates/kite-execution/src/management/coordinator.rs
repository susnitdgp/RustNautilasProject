use super::{cancel, modify};
use crate::{
    mock::{MockBroker, Outcome},
    rate_limit::{Decision, Limiter},
};
use anyhow::{Result, anyhow, ensure};
use kite_journal::{
    actions::{Action, Change, CommandStatus},
    model::Event,
    store::Journal,
};
pub fn prepare(journal: &mut Journal, id: &str, command_id: &str, change: Change) -> Result<()> {
    journal.append(Event::Management {
        id: id.into(),
        action: Action::Prepare {
            command_id: command_id.into(),
            change,
        },
    })?;
    Ok(())
}
pub fn send(
    journal: &mut Journal,
    broker: &mut MockBroker,
    id: &str,
    command_id: &str,
    limiter: &mut Limiter,
) -> Result<()> {
    let order = journal.state().order(id)?;
    let command = order
        .commands
        .entries
        .get(command_id)
        .ok_or_else(|| anyhow!("Unknown management command"))?;
    ensure!(
        command.status == CommandStatus::Prepared,
        "Management command already attempted; retry blocked"
    );
    enum Request {
        Modify(modify::ModifyRequest),
        Cancel(cancel::CancelRequest),
    }
    let request = match command.change {
        Change::Modify {
            quantity,
            limit_price_paise,
        } => Request::Modify(modify::translate(order, quantity, limit_price_paise)?),
        Change::Cancel => Request::Cancel(cancel::translate(order)?),
    };
    ensure!(
        limiter.reserve()? == Decision::Allowed,
        "Management request deferred by shared rate budget"
    );
    journal.append(Event::Management {
        id: id.into(),
        action: Action::Dispatch {
            command_id: command_id.into(),
        },
    })?;
    let response = match request {
        Request::Modify(r) => broker.modify(&r),
        Request::Cancel(r) => broker.cancel(&r),
    };
    let action = match response {
        Outcome::Accepted(broker_id) => Action::Acknowledged {
            command_id: command_id.into(),
            broker_id,
        },
        Outcome::AmbiguousTimeout => Action::Unknown {
            command_id: command_id.into(),
        },
        Outcome::Rejected => Action::Rejected {
            command_id: command_id.into(),
        },
        Outcome::RateLimited { retry_after_ms } => {
            limiter.cooldown(retry_after_ms)?;
            Action::Unknown {
                command_id: command_id.into(),
            }
        }
    };
    journal.append(Event::Management {
        id: id.into(),
        action,
    })?;
    Ok(())
}
/// Synthetic broker confirmation, distinct from the command acknowledgement.
pub fn confirm(journal: &mut Journal, id: &str, command_id: &str, broker_id: &str) -> Result<bool> {
    journal.append(Event::Management {
        id: id.into(),
        action: Action::Confirmed {
            command_id: command_id.into(),
            broker_id: broker_id.into(),
        },
    })
}

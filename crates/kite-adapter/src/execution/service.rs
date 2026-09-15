//! Journal and rate-budget coordination for the guarded Kite transport.
use super::{
    request::Command,
    transport::{KiteOrderTransport, Outcome},
};
use anyhow::{Result, anyhow, ensure};
use kite_execution::rate_limit::{Decision, Limiter};
use kite_journal::{
    actions::{Action, Change, CommandStatus},
    model::{Event, Product, Side},
    state::Status,
    store::Journal,
};
pub async fn place(
    journal: &mut Journal,
    limiter: &mut Limiter,
    transport: &KiteOrderTransport,
    id: &str,
) -> Result<()> {
    ensure!(
        cfg!(feature = "live-orders"),
        "Live Kite order execution is disabled in this build"
    );
    let order = journal.state().order(id)?;
    ensure!(
        order.status == Status::Prepared,
        "Order already attempted; reconcile before retry"
    );
    let command = Command::Place {
        symbol: order.intent.symbol.clone(),
        side: match order.intent.side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        }
        .into(),
        product: match order.intent.product {
            Product::Mis => "MIS",
            Product::Nrml => "NRML",
        }
        .into(),
        quantity: order.intent.quantity,
        price_rupees: order.intent.limit_price_paise / 100,
        tag: order.intent.id.clone(),
    };
    command.validate()?;
    ensure!(
        limiter.reserve()? == Decision::Allowed,
        "Order deferred by shared rate budget"
    );
    journal.append(Event::Dispatch { id: id.into() })?;
    let result = transport.execute(&command).await;
    match result {
        Ok(Outcome::Acknowledged { order_id }) => {
            journal.append(Event::Acknowledged {
                id: id.into(),
                broker_id: order_id,
            })?;
        }
        Ok(Outcome::Rejected) => {
            journal.append(Event::Rejected { id: id.into() })?;
        }
        Ok(Outcome::RateLimited { retry_after_ms }) => {
            journal.append(Event::Unknown { id: id.into() })?;
            limiter.cooldown(retry_after_ms)?;
        }
        Ok(Outcome::Unknown) => {
            journal.append(Event::Unknown { id: id.into() })?;
        }
        Ok(Outcome::SessionExpired) => {
            journal.append(Event::Unknown { id: id.into() })?;
            return Err(anyhow!(
                "Kite session expired; renew credentials and reconcile"
            ));
        }
        Err(_) => {
            journal.append(Event::Unknown { id: id.into() })?;
            return Err(anyhow!(
                "Kite transport failed; submission outcome requires reconciliation"
            ));
        }
    }
    Ok(())
}
pub async fn manage(
    journal: &mut Journal,
    limiter: &mut Limiter,
    transport: &KiteOrderTransport,
    id: &str,
    command_id: &str,
) -> Result<()> {
    ensure!(
        cfg!(feature = "live-orders"),
        "Live Kite order execution is disabled in this build"
    );
    let order = journal.state().order(id)?;
    let pending = order
        .commands
        .entries
        .get(command_id)
        .ok_or_else(|| anyhow!("Unknown management command"))?;
    ensure!(
        pending.status == CommandStatus::Prepared,
        "Management command already attempted"
    );
    kite_journal::actions::validate_change(order, &pending.change)?;
    let broker = order
        .broker_id
        .clone()
        .ok_or_else(|| anyhow!("Missing broker ownership"))?;
    let request = match pending.change {
        Change::Cancel => Command::Cancel { order_id: broker },
        Change::Modify {
            quantity,
            limit_price_paise,
        } => Command::Modify {
            order_id: broker,
            quantity,
            price_rupees: limit_price_paise / 100,
        },
    };
    request.validate()?;
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
    let response = transport.execute(&request).await;
    let mut cooldown = None;
    let mut failed = false;
    let action = match response {
        Ok(Outcome::Acknowledged { order_id }) => Action::Acknowledged {
            command_id: command_id.into(),
            broker_id: order_id,
        },
        Ok(Outcome::Rejected) => Action::Rejected {
            command_id: command_id.into(),
        },
        Ok(Outcome::RateLimited { retry_after_ms }) => {
            cooldown = Some(retry_after_ms);
            Action::Unknown {
                command_id: command_id.into(),
            }
        }
        Ok(Outcome::Unknown) => Action::Unknown {
            command_id: command_id.into(),
        },
        _ => {
            failed = true;
            Action::Unknown {
                command_id: command_id.into(),
            }
        }
    };
    journal.append(Event::Management {
        id: id.into(),
        action,
    })?;
    if let Some(ms) = cooldown {
        limiter.cooldown(ms)?;
    }
    ensure!(
        !failed,
        "Kite management transport/session failed; reconcile before retry"
    );
    Ok(())
}

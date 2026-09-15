//! Persisted modify/cancel command lifecycle, separate from order fill state.
use crate::{
    model::valid_id,
    state::{Order, Status},
};
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    Modify {
        quantity: u32,
        limit_price_paise: i64,
    },
    Cancel,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandStatus {
    Prepared,
    Dispatching,
    Acknowledged,
    Unknown,
    Confirmed,
    Rejected,
}
impl CommandStatus {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Confirmed | Self::Rejected)
    }
}
#[derive(Debug, Clone)]
pub struct Command {
    pub change: Change,
    pub status: CommandStatus,
}
#[derive(Debug, Clone, Default)]
pub struct Commands {
    pub entries: BTreeMap<String, Command>,
    pub modification_attempts: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
pub enum Action {
    Prepare {
        command_id: String,
        change: Change,
    },
    Dispatch {
        command_id: String,
    },
    Acknowledged {
        command_id: String,
        broker_id: String,
    },
    Unknown {
        command_id: String,
    },
    Confirmed {
        command_id: String,
        broker_id: String,
    },
    Rejected {
        command_id: String,
    },
}
impl Action {
    pub fn command_id(&self) -> &str {
        match self {
            Self::Prepare { command_id, .. }
            | Self::Dispatch { command_id }
            | Self::Acknowledged { command_id, .. }
            | Self::Unknown { command_id }
            | Self::Confirmed { command_id, .. }
            | Self::Rejected { command_id } => command_id,
        }
    }
}
pub fn validate_change(order: &Order, change: &Change) -> Result<()> {
    ensure!(
        matches!(order.status, Status::Accepted | Status::PartiallyFilled),
        "Only confirmed open orders can be modified or cancelled"
    );
    ensure!(
        order.broker_id.is_some(),
        "Broker ownership is not established"
    );
    if let Change::Modify {
        quantity,
        limit_price_paise,
    } = change
    {
        let mut intent = order.intent.clone();
        intent.quantity = *quantity;
        intent.limit_price_paise = *limit_price_paise;
        intent.validate()?;
        ensure!(
            *quantity > order.filled,
            "Modified total quantity must exceed filled quantity"
        );
        ensure!(
            order.commands.modification_attempts < 25,
            "Maximum 25 modification attempts reached"
        );
        ensure!(
            *quantity != order.intent.quantity
                || *limit_price_paise != order.intent.limit_price_paise,
            "Modification makes no change"
        );
    }
    Ok(())
}
pub(crate) fn apply(order: &mut Order, action: &Action) -> Result<bool> {
    let id = action.command_id();
    ensure!(valid_id(id), "Invalid management command ID");
    if let Action::Prepare { change, .. } = action {
        validate_change(order, change)?;
        ensure!(
            !order.commands.entries.contains_key(id),
            "Management command ID already exists"
        );
        ensure!(
            order.commands.entries.values().all(|c| c.status.terminal()),
            "Previous management command requires confirmation or reconciliation"
        );
        order.commands.entries.insert(
            id.into(),
            Command {
                change: change.clone(),
                status: CommandStatus::Prepared,
            },
        );
        return Ok(true);
    }
    let command = order
        .commands
        .entries
        .get(id)
        .ok_or_else(|| anyhow!("Unknown management command"))?
        .clone();
    if let Action::Acknowledged { broker_id, .. } | Action::Confirmed { broker_id, .. } = action {
        ensure!(
            order.broker_id.as_ref() == Some(broker_id),
            "Management response broker identity mismatch"
        );
    }
    let next = match action {
        Action::Dispatch { .. } => {
            ensure!(
                command.status == CommandStatus::Prepared,
                "Management command already attempted; retry blocked"
            );
            validate_change(order, &command.change)?;
            if matches!(command.change, Change::Modify { .. }) {
                order.commands.modification_attempts += 1;
            }
            CommandStatus::Dispatching
        }
        Action::Acknowledged { .. } => {
            if matches!(
                command.status,
                CommandStatus::Acknowledged | CommandStatus::Confirmed
            ) {
                return Ok(false);
            }
            ensure!(
                matches!(
                    command.status,
                    CommandStatus::Dispatching | CommandStatus::Unknown
                ),
                "Unexpected management acknowledgement"
            );
            CommandStatus::Acknowledged
        }
        Action::Unknown { .. } => {
            if matches!(
                command.status,
                CommandStatus::Unknown | CommandStatus::Acknowledged | CommandStatus::Confirmed
            ) {
                return Ok(false);
            }
            ensure!(
                command.status == CommandStatus::Dispatching,
                "Unexpected management timeout"
            );
            CommandStatus::Unknown
        }
        Action::Rejected { .. } => {
            if command.status == CommandStatus::Rejected {
                return Ok(false);
            }
            ensure!(
                matches!(
                    command.status,
                    CommandStatus::Dispatching
                        | CommandStatus::Acknowledged
                        | CommandStatus::Unknown
                ),
                "Unexpected management rejection"
            );
            CommandStatus::Rejected
        }
        Action::Confirmed { .. } => {
            if command.status == CommandStatus::Confirmed {
                return Ok(false);
            }
            ensure!(
                matches!(
                    command.status,
                    CommandStatus::Dispatching
                        | CommandStatus::Acknowledged
                        | CommandStatus::Unknown
                ),
                "Confirmation before dispatch or after rejection"
            );
            match command.change {
                Change::Cancel => {
                    ensure!(
                        !matches!(order.status, Status::Prepared | Status::Rejected),
                        "Cancellation conflicts with order state"
                    );
                    // A fill can win the cancellation race; never erase that fill.
                    if order.status != Status::Filled {
                        order.status = Status::Cancelled;
                    }
                }
                Change::Modify {
                    quantity,
                    limit_price_paise,
                } => {
                    ensure!(
                        matches!(order.status, Status::Accepted | Status::PartiallyFilled),
                        "Modification conflicts with terminal order; reconcile"
                    );
                    ensure!(
                        quantity >= order.filled,
                        "Modification confirmation is below recorded fills; reconcile"
                    );
                    order.intent.quantity = quantity;
                    order.intent.limit_price_paise = limit_price_paise;
                    if quantity == order.filled {
                        order.status = Status::Filled;
                    }
                }
            }
            CommandStatus::Confirmed
        }
        Action::Prepare { .. } => unreachable!(),
    };
    order.commands.entries.get_mut(id).unwrap().status = next;
    Ok(true)
}

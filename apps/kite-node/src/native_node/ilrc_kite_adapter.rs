//! ILRC-to-Kite transport adapter. No CLI caller, no order execution by default.
//! Transport results are NOT fills; unknown outcomes require broker reconciliation.
use super::ilrc_order_lifecycle::{Lifecycle, Phase};
use anyhow::{Result, ensure};
use kite_adapter::execution::{
    request::Command,
    transport::{KiteOrderTransport, Outcome},
};

pub struct KiteProductionSubmission<'a> {
    transport: &'a KiteOrderTransport,
}

impl<'a> KiteProductionSubmission<'a> {
    pub fn new(transport: &'a KiteOrderTransport) -> Self {
        Self { transport }
    }

    /// Must only be invoked after the caller atomically journals the reservation
    /// to Redis, verifies broker-flat state and provides working protective-order
    /// management. These prerequisites are NOT implemented in ILRC yet.
    pub async fn submit_disabled_until_reconciled(
        &self,
        lifecycle: &mut Lifecycle,
        order: &Command,
        durable_journal_ready: bool,
        protective_order_management_ready: bool,
        broker_snapshot_reconciled: bool,
    ) -> Result<()> {
        ensure!(
            durable_journal_ready
                && protective_order_management_ready
                && broker_snapshot_reconciled,
            "ILRC production execution prerequisites are not satisfied"
        );
        ensure!(
            matches!(lifecycle.phase, Phase::DispatchUncertain) && !lifecycle.kill_switch,
            "Order was not safely reserved"
        );
        order.validate()?;
        let result = match self.transport.execute(order).await {
            Ok(value) => value,
            Err(err) => {
                lifecycle.trip_kill_switch();
                return Err(err);
            }
        };
        match result {
            Outcome::Acknowledged { order_id } => lifecycle.acknowledged(&order_id),
            _ => {
                lifecycle.trip_kill_switch();
                anyhow::bail!("Kite result uncertain or rejected: halt and reconcile before retry")
            }
        }
    }
}

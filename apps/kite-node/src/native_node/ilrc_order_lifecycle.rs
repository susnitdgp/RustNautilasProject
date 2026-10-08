//! Pure, fail-closed ILRC broker-observation lifecycle model.
//! Does not create clients, access Redis, or submit orders.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Flat,
    DispatchUncertain,
    Accepted,
    PartiallyFilled,
    Unprotected,
    Protected,
    ReviewRequired,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lifecycle {
    pub phase: Phase,
    pub client_order_id: Option<String>,
    pub broker_order_id: Option<String>,
    pub requested: u32,
    pub filled: u32,
    pub stop_confirmed: bool,
    pub seen_trade_ids: BTreeSet<String>,
    pub kill_switch: bool,
}
impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            phase: Phase::Flat,
            client_order_id: None,
            broker_order_id: None,
            requested: 0,
            filled: 0,
            stop_confirmed: false,
            seen_trade_ids: BTreeSet::new(),
            kill_switch: false,
        }
    }
}
impl Lifecycle {
    pub fn reserve(&mut self, id: &str, requested: u32) -> Result<()> {
        ensure!(
            !self.kill_switch && self.phase == Phase::Flat && self.client_order_id.is_none(),
            "Unresolved ILRC order; manual reconciliation required"
        );
        ensure!(
            !id.is_empty() && requested == 1,
            "Invalid ILRC order reservation"
        );
        self.client_order_id = Some(id.into());
        self.requested = requested;
        self.phase = Phase::DispatchUncertain;
        Ok(())
    }
    pub fn acknowledged(&mut self, broker_id: &str) -> Result<()> {
        ensure!(
            self.phase == Phase::DispatchUncertain && !broker_id.is_empty(),
            "Unexpected broker acknowledgement"
        );
        self.broker_order_id = Some(broker_id.into());
        self.phase = Phase::Accepted;
        Ok(())
    }
    pub fn observed_fill(&mut self, broker_id: &str, trade_id: &str, qty: u32) -> Result<()> {
        ensure!(
            self.broker_order_id.as_deref() == Some(broker_id),
            "Broker order identity mismatch"
        );
        ensure!(
            matches!(
                self.phase,
                Phase::Accepted | Phase::PartiallyFilled | Phase::Unprotected | Phase::Protected
            ),
            "Fill arrived in forbidden lifecycle phase"
        );
        ensure!(!trade_id.is_empty() && qty > 0, "Invalid fill observation");
        if self.seen_trade_ids.contains(trade_id) {
            return Ok(());
        }
        ensure!(
            self.filled
                .checked_add(qty)
                .is_some_and(|n| n <= self.requested),
            "Overfill or impossible fill quantity"
        );
        self.filled += qty;
        self.seen_trade_ids.insert(trade_id.into());
        self.phase = if self.filled < self.requested {
            Phase::PartiallyFilled
        } else {
            Phase::Unprotected
        };
        Ok(())
    }
    pub fn protective_stop_confirmed(&mut self) -> Result<()> {
        ensure!(
            self.phase == Phase::Unprotected && self.filled == self.requested,
            "Cannot claim stop before complete entry fill"
        );
        self.stop_confirmed = true;
        self.phase = Phase::Protected;
        Ok(())
    }
    pub fn reconcile(
        &mut self,
        broker_position: u32,
        broker_order_unresolved: bool,
        protective_order_present: bool,
    ) -> Result<()> {
        if broker_position != self.filled
            || (self.filled > 0 && !protective_order_present)
            || (broker_order_unresolved && self.phase == Phase::Flat)
        {
            self.phase = Phase::ReviewRequired;
            self.kill_switch = true;
            anyhow::bail!(
                "Broker/ILRC position or protective order mismatch: manual review required"
            );
        }
        if self.filled > 0 && self.stop_confirmed && !protective_order_present {
            self.phase = Phase::ReviewRequired;
            self.kill_switch = true;
            anyhow::bail!("Protective order disappeared");
        }
        Ok(())
    }
    pub fn trip_kill_switch(&mut self) {
        self.kill_switch = true;
    }
    pub fn can_enter(&self) -> bool {
        self.phase == Phase::Flat && !self.kill_switch && self.client_order_id.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acknowledgement_does_not_mean_fill_or_protection() {
        let mut x = Lifecycle::default();
        x.reserve("client-1", 1).unwrap();
        x.acknowledged("broker-1").unwrap();
        assert_eq!(x.filled, 0);
        assert!(!x.stop_confirmed);
        assert!(!x.can_enter());
    }
    #[test]
    fn fill_identity_duplicate_and_stop_confirmation() {
        let mut x = Lifecycle::default();
        x.reserve("a", 1).unwrap();
        x.acknowledged("b").unwrap();
        assert!(x.observed_fill("wrong", "t", 1).is_err());
        x.observed_fill("b", "t", 1).unwrap();
        x.observed_fill("b", "t", 1).unwrap();
        assert_eq!(x.filled, 1);
        assert!(!x.can_enter());
        x.protective_stop_confirmed().unwrap();
        assert_eq!(x.phase, Phase::Protected);
    }
    #[test]
    fn replayed_checkpoint_retains_idempotency() {
        let mut x = Lifecycle::default();
        x.reserve("a", 1).unwrap();
        x.acknowledged("b").unwrap();
        x.observed_fill("b", "t", 1).unwrap();
        let mut y: Lifecycle = serde_json::from_slice(&serde_json::to_vec(&x).unwrap()).unwrap();
        y.observed_fill("b", "t", 1).unwrap();
        assert_eq!(y.filled, 1);
        assert!(y.reserve("a", 1).is_err());
    }
    #[test]
    fn mismatch_latches_kill_and_review() {
        let mut x = Lifecycle::default();
        x.reserve("a", 1).unwrap();
        x.acknowledged("b").unwrap();
        x.observed_fill("b", "t", 1).unwrap();
        assert!(x.reconcile(1, false, false).is_err());
        assert!(x.kill_switch);
        assert_eq!(x.phase, Phase::ReviewRequired);
    }
    #[test]
    fn partial_fill_rejects_premature_stop_confirmation() {
        let mut x = Lifecycle::default();
        x.reserve("a", 2).unwrap_err();
        let mut y = Lifecycle::default();
        y.reserve("a", 1).unwrap();
        y.acknowledged("b").unwrap();
        assert!(y.observed_fill("b", "t", 2).is_err());
        assert!(y.protective_stop_confirmed().is_err());
    }
    #[test]
    fn timeout_remains_uncertain_and_fails_closed() {
        let mut x = Lifecycle::default();
        x.reserve("a", 1).unwrap();
        assert!(x.reserve("b", 1).is_err());
        assert!(!x.can_enter());
    }
}

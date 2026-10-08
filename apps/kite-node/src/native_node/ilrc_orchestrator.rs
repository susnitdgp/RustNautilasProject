//! ILRC execution orchestration core: broker observations control transitions.
//! No network, credentials, Redis mutation, or live CLI wiring.
use super::{
    ilrc_backtest::EntryEvent,
    ilrc_order_intent::entry_command,
    ilrc_order_lifecycle::{Lifecycle, Phase},
};
use anyhow::{Result, ensure};
use kite_adapter::execution::{request::Command, transport::Outcome};
use serde::{Deserialize, Serialize};

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Orchestrator {
    pub state: Lifecycle,
    pub protective_order_id: Option<String>,
    pub stop_trigger: i64,
}
impl Orchestrator {
    pub fn prepare(
        &mut self,
        signal: &EntryEvent,
        symbol: &str,
        now: i64,
        broker_flat: bool,
        reconciled: bool,
    ) -> Result<Command> {
        ensure!(self.state.can_enter(), "Unresolved previous ILRC trade");
        let cmd = entry_command(
            signal,
            symbol,
            1,
            now,
            !broker_flat,
            reconciled,
            self.state.kill_switch,
        )?;
        let id = format!(
            "ILRC:{}:{}:{}",
            signal.setup, signal.entry_time, signal.side
        );
        ensure!(
            signal.stop.is_finite() && signal.stop.fract() == 0.0 && signal.stop > 0.0,
            "Stop tick precision not confirmed"
        );
        self.state.reserve(&id, 1)?;
        self.stop_trigger = signal.stop as i64;
        Ok(cmd)
    }
    pub fn entry_receipt(&mut self, result: Outcome) -> Result<()> {
        match result {
            Outcome::Acknowledged { order_id } => self.state.acknowledged(&order_id),
            _ => {
                self.state.trip_kill_switch();
                anyhow::bail!("Entry outcome uncertain: reconcile first")
            }
        }
    }
    pub fn entry_fill(&mut self, order_id: &str, trade_id: &str, qty: u32) -> Result<()> {
        self.state.observed_fill(order_id, trade_id, qty)
    }
    pub fn stop_intent(&self, symbol: &str, entry_side: &str) -> Result<Command> {
        ensure!(
            self.state.phase == Phase::Unprotected && self.state.filled == self.state.requested,
            "No fully filled unprotected entry"
        );
        let side = match entry_side {
            "LONG" => "SELL",
            "SHORT" => "BUY",
            _ => anyhow::bail!("Invalid entry side"),
        };
        let cmd = Command::ProtectiveStopMarket {
            symbol: symbol.into(),
            side: side.into(),
            product: "MIS".into(),
            quantity: self.state.filled,
            trigger_price_rupees: self.stop_trigger,
            tag: "ILRCSTOPSTAGED".into(),
            market_protection: -1,
        };
        cmd.validate()?;
        Ok(cmd)
    }
    pub fn stop_receipt(&mut self, result: Outcome) -> Result<()> {
        match result {
            Outcome::Acknowledged { order_id } => {
                ensure!(
                    self.state.phase == Phase::Unprotected,
                    "Unexpected stop acknowledgement"
                );
                self.protective_order_id = Some(order_id);
                Ok(())
            }
            _ => {
                self.state.trip_kill_switch();
                anyhow::bail!("Protective stop submission unknown: manual review")
            }
        }
    }
    pub fn stop_observed(
        &mut self,
        order_id: &str,
        qty: u32,
        trigger: i64,
        active: bool,
    ) -> Result<()> {
        if self.protective_order_id.as_deref() != Some(order_id)
            || qty != self.state.filled
            || trigger != self.stop_trigger
            || !active
        {
            self.state.trip_kill_switch();
            anyhow::bail!("Stop broker observation mismatch")
        }
        self.state.protective_stop_confirmed()
    }
    pub fn break_even_modify_intent(&self, new_trigger: i64) -> Result<Command> {
        ensure!(
            self.state.phase == Phase::Protected && !self.state.kill_switch,
            "Cannot modify an unconfirmed protective stop"
        );
        let order_id = self
            .protective_order_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Stop broker ID missing"))?;
        ensure!(new_trigger > 0, "Invalid break-even stop");
        let cmd = Command::ModifyProtectiveStop {
            order_id: order_id.clone(),
            quantity: self.state.filled,
            trigger_price_rupees: new_trigger,
            market_protection: -1,
        };
        cmd.validate()?;
        Ok(cmd)
    }
    pub fn break_even_observed(
        &mut self,
        order_id: &str,
        new_trigger: i64,
        active: bool,
    ) -> Result<()> {
        if self.state.phase != Phase::Protected
            || self.protective_order_id.as_deref() != Some(order_id)
            || !active
            || new_trigger <= 0
        {
            self.state.trip_kill_switch();
            anyhow::bail!("Break-even stop modification unconfirmed");
        }
        self.stop_trigger = new_trigger;
        Ok(())
    }
    pub fn reconcile(&mut self, position: u32, stop_exists: bool) -> Result<()> {
        self.state.reconcile(position, false, stop_exists)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn event() -> EntryEvent {
        EntryEvent {
            setup: "B",
            entry_time: "2026-10-07T12:00:00+05:30".into(),
            observed_at: "2026-10-07T12:03:00+05:30".into(),
            side: "LONG",
            entry: 8700.0,
            stop: 8690.0,
            target: 8730.0,
        }
    }
    fn time() -> i64 {
        chrono::DateTime::parse_from_rfc3339(&event().observed_at)
            .unwrap()
            .timestamp()
    }
    #[test]
    fn full_mock_order_and_protective_lifecycle() {
        let mut o = Orchestrator::default();
        let c = o
            .prepare(&event(), "CRUDEOIL26OCTFUT", time(), true, true)
            .unwrap();
        assert!(matches!(c, Command::ProtectedMarket { .. }));
        o.entry_receipt(Outcome::Acknowledged {
            order_id: "101".into(),
        })
        .unwrap();
        assert!(o.stop_intent("CRUDEOIL26OCTFUT", "LONG").is_err());
        o.entry_fill("101", "trade-1", 1).unwrap();
        let stop = o.stop_intent("CRUDEOIL26OCTFUT", "LONG").unwrap();
        assert!(matches!(stop, Command::ProtectiveStopMarket { .. }));
        o.stop_receipt(Outcome::Acknowledged {
            order_id: "202".into(),
        })
        .unwrap();
        assert!(o.state.phase == Phase::Unprotected);
        o.stop_observed("202", 1, 8690, true).unwrap();
        o.reconcile(1, true).unwrap();
        assert!(
            o.prepare(&event(), "CRUDEOIL26OCTFUT", time(), true, true)
                .is_err()
        );
    }
    #[test]
    fn break_even_requires_protected_state_and_broker_confirmation() {
        let mut o = Orchestrator::default();
        assert!(o.break_even_modify_intent(8700).is_err());
        o.prepare(&event(), "CRUDEOIL26OCTFUT", time(), true, true)
            .unwrap();
        o.entry_receipt(Outcome::Acknowledged {
            order_id: "101".into(),
        })
        .unwrap();
        o.entry_fill("101", "t", 1).unwrap();
        o.stop_receipt(Outcome::Acknowledged {
            order_id: "202".into(),
        })
        .unwrap();
        o.stop_observed("202", 1, 8690, true).unwrap();
        assert!(matches!(
            o.break_even_modify_intent(8700).unwrap(),
            Command::ModifyProtectiveStop { .. }
        ));
        o.break_even_observed("202", 8700, true).unwrap();
        assert_eq!(o.stop_trigger, 8700);
    }
    #[test]
    fn missing_or_wrong_stop_latches_kill() {
        let mut o = Orchestrator::default();
        o.prepare(&event(), "CRUDEOIL26OCTFUT", time(), true, true)
            .unwrap();
        o.entry_receipt(Outcome::Acknowledged {
            order_id: "101".into(),
        })
        .unwrap();
        o.entry_fill("101", "t", 1).unwrap();
        o.stop_receipt(Outcome::Acknowledged {
            order_id: "202".into(),
        })
        .unwrap();
        assert!(o.stop_observed("202", 0, 8690, true).is_err());
        assert!(o.state.kill_switch);
    }
    #[test]
    fn uncertain_ack_latches_kill() {
        let mut o = Orchestrator::default();
        o.prepare(&event(), "CRUDEOIL26OCTFUT", time(), true, true)
            .unwrap();
        assert!(o.entry_receipt(Outcome::Unknown).is_err());
        assert!(o.state.kill_switch);
    }
}

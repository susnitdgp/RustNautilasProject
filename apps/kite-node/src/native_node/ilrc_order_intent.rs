//! Typed ILRC-to-Kite order-intent boundary; no transport invocation.
//! Deliberately not exposed as a live CLI order submission command.
use super::ilrc_backtest::EntryEvent;
use anyhow::{Result, ensure};
use kite_adapter::execution::request::Command;

pub fn entry_command(
    signal: &EntryEvent,
    symbol: &str,
    quantity: u32,
    now_epoch: i64,
    has_position: bool,
    broker_reconciled: bool,
    kill_switch: bool,
) -> Result<Command> {
    ensure!(!kill_switch, "ILRC kill switch active");
    ensure!(
        broker_reconciled && !has_position,
        "ILRC entry requires reconciled flat broker position"
    );
    ensure!(quantity == 1, "ILRC supports exactly one contract");
    let observed = chrono::DateTime::parse_from_rfc3339(&signal.observed_at)?.timestamp();
    ensure!(
        now_epoch >= observed && now_epoch - observed < 180,
        "ILRC entry event not fresh and post-close"
    );
    let side = match signal.side {
        "LONG" => "BUY",
        "SHORT" => "SELL",
        _ => anyhow::bail!("Invalid ILRC side"),
    };
    ensure!(
        [signal.entry, signal.stop, signal.target]
            .iter()
            .all(|x| x.is_finite() && *x > 0.0),
        "Invalid ILRC prices"
    );
    ensure!(
        if side == "BUY" {
            signal.stop < signal.entry && signal.target > signal.entry
        } else {
            signal.stop > signal.entry && signal.target < signal.entry
        },
        "Invalid stop or target direction"
    );
    ensure!(matches!(signal.setup, "A" | "B"), "Unknown ILRC setup");
    let command = Command::ProtectedMarket {
        symbol: symbol.into(),
        side: side.into(),
        product: "MIS".into(),
        quantity,
        tag: "ILRCSTAGEDONLY".into(),
        market_protection: -1,
    };
    command.validate()?;
    Ok(command)
}

/// Narrow Kite command submission seam. The live implementation is deliberately
/// absent until broker-side protective orders and reconciliation are operational.
pub trait KiteOrderGateway {
    fn submit(&mut self, command: Command) -> Result<()>;
}

/// An explicitly non-networking gateway for integration tests.
#[derive(Default)]
pub struct DryRunKiteGateway {
    pub accepted: usize,
}
impl KiteOrderGateway for DryRunKiteGateway {
    fn submit(&mut self, command: Command) -> Result<()> {
        command.validate()?;
        self.accepted += 1;
        Ok(())
    }
}

/// Build a Kite-compatible command and dispatch it through an injected gateway.
/// The CLI does not create a live gateway or connect to broker mutations.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_entry<G: KiteOrderGateway>(
    gateway: &mut G,
    signal: &EntryEvent,
    symbol: &str,
    quantity: u32,
    now_epoch: i64,
    has_position: bool,
    broker_reconciled: bool,
    kill_switch: bool,
) -> Result<()> {
    let command = entry_command(
        signal,
        symbol,
        quantity,
        now_epoch,
        has_position,
        broker_reconciled,
        kill_switch,
    )?;
    gateway.submit(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn signal() -> EntryEvent {
        EntryEvent {
            setup: "B",
            entry_time: "2026-10-07T12:00:00+05:30".into(),
            observed_at: "2026-10-07T12:03:00+05:30".into(),
            side: "LONG",
            entry: 100.0,
            stop: 95.0,
            target: 115.0,
        }
    }
    fn ts() -> i64 {
        chrono::DateTime::parse_from_rfc3339(&signal().observed_at)
            .unwrap()
            .timestamp()
    }
    #[test]
    fn kite_gateway_dispatch_is_mock_only_and_rejects_unsafe_entries() {
        let mut gateway = DryRunKiteGateway::default();
        dispatch_entry(
            &mut gateway,
            &signal(),
            "CRUDEOIL26OCTFUT",
            1,
            ts(),
            false,
            true,
            false,
        )
        .unwrap();
        assert_eq!(gateway.accepted, 1);
        assert!(
            dispatch_entry(
                &mut gateway,
                &signal(),
                "CRUDEOIL26OCTFUT",
                1,
                ts(),
                true,
                true,
                false
            )
            .is_err()
        );
        assert!(
            dispatch_entry(
                &mut gateway,
                &signal(),
                "CRUDEOIL26OCTFUT",
                1,
                ts(),
                false,
                true,
                true
            )
            .is_err()
        );
        assert_eq!(gateway.accepted, 1);
    }
    #[test]
    fn constructs_validated_market_intent_without_submission() {
        assert!(entry_command(&signal(), "CRUDEOIL26OCTFUT", 1, ts(), false, true, false).is_ok());
    }
    #[test]
    fn requires_flat_and_reconciled() {
        assert!(entry_command(&signal(), "CRUDEOIL26OCTFUT", 1, ts(), true, true, false).is_err());
        assert!(
            entry_command(&signal(), "CRUDEOIL26OCTFUT", 1, ts(), false, false, false).is_err()
        );
    }
    #[test]
    fn stale_future_and_killed_rejected() {
        assert!(
            entry_command(
                &signal(),
                "CRUDEOIL26OCTFUT",
                1,
                ts() - 1,
                false,
                true,
                false
            )
            .is_err()
        );
        assert!(
            entry_command(
                &signal(),
                "CRUDEOIL26OCTFUT",
                1,
                ts() + 181,
                false,
                true,
                false
            )
            .is_err()
        );
        assert!(entry_command(&signal(), "CRUDEOIL26OCTFUT", 1, ts(), false, true, true).is_err());
    }
    #[test]
    fn quantity_and_protection_checked() {
        assert!(entry_command(&signal(), "CRUDEOIL26OCTFUT", 2, ts(), false, true, false).is_err());
        let mut invalid = signal();
        invalid.stop = 101.0;
        assert!(entry_command(&invalid, "CRUDEOIL26OCTFUT", 1, ts(), false, true, false).is_err());
    }
}

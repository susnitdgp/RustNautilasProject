//! End-to-end broker-protocol *simulation*: validated Kite order intent -> mock acknowledgement ->
//! trade observations -> protective-order confirmation -> reconciliation. No network or credentials.
use super::{
    ilrc_backtest::EntryEvent,
    ilrc_order_intent::{DryRunKiteGateway, dispatch_entry},
    ilrc_order_lifecycle::{Lifecycle, Phase},
};
use anyhow::{Result, ensure};
use kite_adapter::execution::transport::Outcome;

pub fn run() -> Result<()> {
    let signal = EntryEvent {
        setup: "A",
        entry_time: "2026-10-07T12:00:00+05:30".into(),
        observed_at: "2026-10-07T12:03:00+05:30".into(),
        side: "LONG",
        entry: 100.0,
        stop: 95.0,
        target: 115.0,
    };
    let ts = chrono::DateTime::parse_from_rfc3339(&signal.observed_at)?.timestamp();
    let mut state = Lifecycle::default();
    let mut gateway = DryRunKiteGateway::default();
    state.reserve("ilrc-a-20261007", 1)?;
    dispatch_entry(
        &mut gateway,
        &signal,
        "CRUDEOIL26OCTFUT",
        1,
        ts,
        false,
        true,
        false,
    )?;
    ensure!(gateway.accepted == 1, "Mock order intent not accepted");
    observe_outcome(
        &mut state,
        Outcome::Acknowledged {
            order_id: "mock-order-01".into(),
        },
    )?;
    ensure!(
        state.phase == Phase::Accepted && state.filled == 0,
        "HTTP acknowledgement cannot imply fill"
    );
    state.observed_fill("mock-order-01", "mock-trade-01", 1)?;
    state.observed_fill("mock-order-01", "mock-trade-01", 1)?;
    ensure!(
        state.phase == Phase::Unprotected && state.filled == 1,
        "Unexpected fill state"
    );
    state.protective_stop_confirmed()?;
    state.reconcile(1, false, true)?;
    let bytes = serde_json::to_vec(&state)?;
    let restored: Lifecycle = serde_json::from_slice(&bytes)?;
    ensure!(
        restored.phase == Phase::Protected && !restored.can_enter(),
        "Restart violated protected single-position state"
    );
    println!(
        "{}",
        serde_json::json!({"event":"ilrc_mock_broker_lifecycle","status":"PASS","gateway_accepted":gateway.accepted,"phase":"Protected","confirmed_fills":restored.filled,"duplicate_fill_ignored":true,"restored_state":true,"real_order_requests":0,"network_access":false,"redis_integration":false})
    );
    Ok(())
}
fn observe_outcome(state: &mut Lifecycle, result: Outcome) -> Result<()> {
    match result {
        Outcome::Acknowledged { order_id } => state.acknowledged(&order_id),
        _ => {
            state.trip_kill_switch();
            anyhow::bail!("Broker mutation result not acknowledged; reconciliation required")
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uncertain_ack_halts_and_prevents_retry() {
        let mut state = Lifecycle::default();
        state.reserve("a", 1).unwrap();
        assert!(observe_outcome(&mut state, Outcome::Unknown).is_err());
        assert!(state.kill_switch);
        assert!(!state.can_enter());
    }
    #[test]
    fn rejection_halts_and_prevents_retry() {
        let mut state = Lifecycle::default();
        state.reserve("a", 1).unwrap();
        assert!(observe_outcome(&mut state, Outcome::Rejected).is_err());
        assert!(state.kill_switch);
    }
    #[test]
    fn full_mock_lifecycle() {
        run().unwrap();
    }
}

//! Fail-closed ILRC live integration readiness. No broker or execution client is loaded.
use anyhow::{Result, ensure};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    mode: String,
    live_orders_enabled: bool,
    max_open_positions: u32,
    setup_a_priority_on_tie: bool,
    require_broker_reconciliation: bool,
    require_broker_protective_stop: bool,
    require_duplicate_order_protection: bool,
    require_kill_switch: bool,
    require_causal_entry_signals: bool,
}

pub fn check(strategy_path: &str, broker_path: &str, policy_path: &str) -> Result<()> {
    let strategy = super::ilrc_config::Selection::load(strategy_path)?;
    let broker: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(broker_path)?)?;
    let policy: Policy = serde_json::from_str(&std::fs::read_to_string(policy_path)?)?;
    ensure!(
        policy.mode == "integration_dry_run",
        "Only integration_dry_run is permitted"
    );
    ensure!(
        !policy.live_orders_enabled && !strategy.live_orders_enabled,
        "Live orders are not authorized"
    );
    ensure!(
        broker.get("live_orders_enabled") == Some(&serde_json::Value::Bool(false)),
        "Broker live gate must be explicitly false"
    );
    ensure!(
        policy.max_open_positions == 1 && policy.setup_a_priority_on_tie,
        "Position arbitration must remain one-at-a-time with A priority"
    );
    ensure!(
        policy.require_broker_reconciliation
            && policy.require_broker_protective_stop
            && policy.require_duplicate_order_protection
            && policy.require_kill_switch
            && policy.require_causal_entry_signals,
        "Mandatory safety requirements cannot be relaxed"
    );
    println!(
        "{}",
        serde_json::json!({
            "event":"ilrc_live_integration_readiness",
            "configuration_valid":true,
            "live_execution_ready":false,
            "real_orders_enabled":false,
            "execution_client_loaded":false,
            "broker_orders_sent":false,
            "mode":policy.mode,
            "instrument":strategy.instrument,
            "pending_implementation":["causal_incremental_entry_signals","broker_order_lifecycle","protective_stops_and_break_even_updates","position_and_order_reconciliation","idempotency_and_single_position_lock","kill_switch_and_daily_risk_limits","mock_execution_and_restart_recovery_tests"]
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integration_policy_is_fail_closed() {
        let p: Policy = serde_json::from_str(include_str!(
            "../../../../config/ilrc-live-integration.json"
        ))
        .unwrap();
        assert_eq!(p.mode, "integration_dry_run");
        assert!(!p.live_orders_enabled);
        assert_eq!(p.max_open_positions, 1);
        assert!(p.setup_a_priority_on_tie);
    }
}

pub mod backtest_report;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod bar_timing;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
pub mod catalog;
pub mod cli;
mod portfolio;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
pub mod data;
mod execution_session;
pub mod full_codec;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod history_revision;
mod iatf_multitimeframe;
mod iatf_recorder;
#[allow(dead_code)]
mod iatf_replay;
#[allow(dead_code)]
mod iatf_research;
mod iatf_september;
mod iatf_trade_research;
mod ilrc_backtest;
mod ilrc_broker_replay;
mod ilrc_causal_audit;
mod ilrc_config;
mod ilrc_continuation_backtest;
mod ilrc_dashboard;
#[allow(dead_code)]
mod ilrc_kite_adapter;
#[allow(dead_code)]
mod ilrc_live_actor;
mod ilrc_live_dashboard;
mod ilrc_live_readiness;
mod ilrc_mock_execution;
mod ilrc_native_runner;
#[allow(dead_code)]
mod ilrc_nautilus_bridge;
#[allow(dead_code)]
mod ilrc_orchestrator;
#[allow(dead_code)]
mod ilrc_order_intent;
#[allow(dead_code)]
mod ilrc_order_lifecycle;
#[allow(dead_code)]
mod ilrc_redis_state;
mod ilrc_shadow;
mod ilrc_timed_mock;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
pub mod lifecycle;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod live_bars;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod live_control;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod live_data;
mod ws_candles;
mod ws_validation;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod live_lease;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod owner_monitor;
pub mod persistence;
pub mod recovery;
pub mod redis_cache;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod session_calendar;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod slack_alerts;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
pub mod status;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod strategy_session;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod synthetic;
#[cfg(test)]
mod test_support;

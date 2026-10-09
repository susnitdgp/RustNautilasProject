pub mod backtest_report;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod bar_timing;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
pub mod catalog;
pub mod cli;
mod portfolio;
#[allow(dead_code)]
pub mod data;
mod execution_session;
pub mod full_codec;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
pub mod lifecycle;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod live_bars;
// Legacy shared infrastructure kept for historical tooling and tests.
#[allow(dead_code)]
mod live_control;
mod ws_candles;
mod live_backfill;
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
mod synthetic;
#[cfg(test)]
mod test_support;
mod sats_backtest;
mod sniper_backtest;
mod sniper_config;
mod sniper_live;
mod sniper_strategy;
pub mod sats_config;
mod sats_dashboard;
mod sats_live;
mod sats_strategy;
mod sats_trail;

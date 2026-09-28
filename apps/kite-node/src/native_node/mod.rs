pub mod backtest_report;
pub mod catalog;
pub mod cli;
mod dashboard;
pub mod data;
pub mod full_codec;
pub mod lifecycle;
pub mod persistence;
pub mod recovery;
pub mod redis_cache;
pub mod squeeze_momentum_actor;
pub mod status;
mod synthetic;

mod production;

mod bar_timing;
mod live_bars;
mod live_control;
mod live_data;
mod live_lease;
mod squeeze_momentum_live_runner;

mod squeeze_momentum_terminal;

mod history_revision;

mod execution_session;

mod owner_monitor;

mod slack_alerts;

mod session_calendar;

mod strategy_session;

mod squeeze_momentum_recorder;

#[cfg(test)]
mod test_support;

mod squeeze_momentum_backtest;
mod squeeze_momentum_indicator;
mod squeeze_momentum_strategy;

pub mod backtest_report;
pub mod catalog;
pub mod cli;
pub mod data;
pub mod full_codec;
pub mod lifecycle;
pub mod persistence;
pub mod recovery;
pub mod redis_cache;
pub mod status;
mod synthetic;
pub mod trend_ribbon_actor;

mod production;

mod bar_timing;
mod live_bars;
mod live_control;
mod live_data;
mod live_lease;
mod trend_ribbon_live_runner;

mod trend_ribbon_terminal;

mod history_revision;

mod execution_session;

mod owner_monitor;

mod slack_alerts;

mod session_calendar;

mod strategy_session;

mod trend_ribbon;
mod trend_ribbon_backtest;
mod trend_ribbon_realtime;
mod trend_ribbon_recorder;
mod trend_ribbon_replay;

#[cfg(test)]
mod test_support;

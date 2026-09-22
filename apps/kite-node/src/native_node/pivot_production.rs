//! Offline Pivot production configuration check. No credentials, broker I/O or Redis writes.
use super::{data, pivot_session, production::Selection};
use anyhow::{Result, ensure};

pub fn check(config: &str, broker: &str) -> Result<()> {
    let selection = Selection::load(config)?;
    ensure!(
        selection.pivot_point.is_some() || selection.trend_ribbon.is_some(),
        "This command requires a production-capable Pivot or Trend Ribbon JSON selection"
    );
    let settings = selection.broker_settings(broker)?;
    let now = data::now();
    let date = pivot_session::date(now);
    let window = selection.execution_bounds(date, true).ok();
    let ist = |ns: u64| {
        chrono::DateTime::from_timestamp_nanos(ns as i64)
            .with_timezone(&chrono::FixedOffset::east_opt(19_800).unwrap())
            .to_rfc3339()
    };
    println!(
        "{}",
        serde_json::json!({
            "event": "strategy_production_configuration_check",
            "configuration_valid": true,
            "strategy": selection.strategy,
            "instrument": selection.instrument,
            "instrument_token": settings.instrument_token,
            "product": settings.product,
            "market_protection": settings.market_protection,
            "configured_live_orders_enabled": settings.live_orders_enabled,
            "session_start_ist": window.map(|(start,_)|ist(start)),
            "square_off_ist": window.map(|(_,end)|ist(end)),
            "can_start_now_by_calendar": selection.production_duration(now).is_ok(),
            "market_exit_buffer_seconds": super::supertrend_session::EXIT_BUFFER_SECONDS,
            "engine_started": false, "account_checked": false, "instrument_master_checked": false,
            "broker_orders_sent": false
        })
    );
    Ok(())
}

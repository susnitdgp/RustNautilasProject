//! Read-only Kite full-tick recorder for Trend Ribbon realtime parity work.
//!
//! This module never creates a strategy, RiskEngine, execution client or order.
//! It records complete Kite full packets into the native Parquet catalog only.
use anyhow::{Context, Result, ensure};
use kite_adapter::{
    data::full_tick::KiteFullTick,
    mapping::quotes,
    websocket::supervisor::{self, FeedEvent},
};
use nautilus_core::UUID4;
use nautilus_model::instruments::InstrumentAny;
use std::{path::PathBuf, sync::Arc, time::Duration};

fn collect_snapshot(
    snapshot: kite_adapter::mapping::market_data::Snapshot,
    instrument: &nautilus_model::instruments::FuturesContract,
) -> Result<Option<KiteFullTick>> {
    if !snapshot.source_fresh {
        return Ok(None);
    }
    let complete = snapshot
        .raw
        .as_ref()
        .is_some_and(|raw| raw.full.is_some() && raw.quote_fields.is_some());
    if !complete {
        return Ok(None);
    }
    let Some(quote) = quotes::map(&snapshot, instrument)? else {
        return Ok(None);
    };
    Ok(Some(KiteFullTick { snapshot, quote }))
}

pub fn run(config: &str, seconds: u64) -> Result<()> {
    ensure!(
        (60..=86_400).contains(&seconds),
        "Recorder duration must be 60..86400 seconds"
    );
    let selection = super::production::Selection::load(config)?;
    ensure!(
        selection.trend_ribbon.is_some(),
        "Recorder requires a Trend Ribbon selection"
    );

    let now = chrono::Utc::now();
    let date = now
        .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"))
        .date_naive();
    let master = kite_adapter::http::instruments::download()?;
    let report = selection.resolve(&master, date)?;
    let instrument =
        kite_adapter::instruments::contract::build(&report, super::data::now().into())?;
    let credentials = Arc::new(kite_adapter::credentials::redis::load_from_env()?);

    let id = UUID4::new();
    let path = PathBuf::from(format!("data/native-catalog/{id}"));
    ensure!(!path.exists(), "Recorder catalog path already exists");

    println!(
        "{}",
        serde_json::json!({
            "event":"trend_ribbon_tick_recording_started",
            "namespace":id.to_string(),
            "instrument":instrument.id.to_string(),
            "instrument_token":report.instrument_token,
            "duration_seconds":seconds,
            "catalog":path,
            "market_data_source":"kite_live",
            "execution_client_created":false,
            "strategy_created":false,
            "broker_orders_accessed":false,
            "live_orders_enabled":false
        })
    );

    let mut ticks = Vec::<KiteFullTick>::new();
    let mut mapping_error: Option<String> = None;
    let mut connected_events = 0u64;
    let mut gap_events = 0u64;
    let summary = tokio::runtime::Runtime::new()?.block_on(async {
        supervisor::observe(
            &credentials,
            report.instrument_token,
            Duration::from_secs(seconds),
            |event| match event {
                FeedEvent::Connected { .. } => connected_events += 1,
                FeedEvent::Gap { .. } => gap_events += 1,
                FeedEvent::Snapshot(snapshot) => {
                    if mapping_error.is_some() {
                        return;
                    }
                    match collect_snapshot(*snapshot, &instrument) {
                        Ok(Some(tick)) => ticks.push(tick),
                        Ok(None) => {}
                        Err(error) => mapping_error = Some(error.to_string()),
                    }
                }
            },
        )
        .await
    })?;

    if let Some(error) = mapping_error {
        anyhow::bail!("Kite full-tick mapping failed: {error}");
    }
    ensure!(
        !ticks.is_empty(),
        "No complete fresh Kite full packets were recorded"
    );
    ensure!(
        summary.full_ticks as usize >= ticks.len(),
        "Recorded packet count exceeds WebSocket summary"
    );

    super::catalog::write_full(
        &path,
        InstrumentAny::FuturesContract(instrument.clone()),
        &ticks,
    )?;
    let replayed = super::catalog::read_full(&path)?;
    ensure!(
        replayed == ticks,
        "Recorder catalog round-trip differs from captured packets"
    );

    let first = ticks.first().context("recording unexpectedly empty")?;
    let last = ticks.last().context("recording unexpectedly empty")?;
    let first_trade = first
        .snapshot
        .raw
        .as_ref()
        .and_then(|raw| raw.full.as_ref())
        .and_then(|full| full.last_trade_timestamp);
    let last_trade = last
        .snapshot
        .raw
        .as_ref()
        .and_then(|raw| raw.full.as_ref())
        .and_then(|full| full.last_trade_timestamp);

    println!(
        "{}",
        serde_json::json!({
            "event":"trend_ribbon_tick_recording_complete",
            "namespace":id.to_string(),
            "instrument":instrument.id.to_string(),
            "instrument_token":report.instrument_token,
            "catalog":path,
            "captured_full_packets":ticks.len(),
            "websocket_full_packets":summary.full_ticks,
            "websocket_ticks":summary.ticks,
            "connection_generations":summary.connection_generations,
            "connected_events":connected_events,
            "gap_events":gap_events,
            "first_last_trade_timestamp":first_trade,
            "last_last_trade_timestamp":last_trade,
            "first_receive_ns":first.quote.ts_init.as_u64(),
            "last_receive_ns":last.quote.ts_init.as_u64(),
            "catalog_roundtrip_verified":true,
            "execution_client_created":false,
            "strategy_created":false,
            "broker_orders_accessed":false,
            "live_orders_enabled":false
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_fresh_snapshot_becomes_full_tick_without_execution() {
        let (instrument, ts) = crate::paper_flow::simulation::fixture().unwrap();
        let snapshot =
            crate::paper_flow::simulation::full_snapshot_for(144_870_151, 6123, ts.as_u64(), 1);
        let tick = collect_snapshot(snapshot.clone(), &instrument)
            .unwrap()
            .expect("complete tick");
        assert_eq!(tick.snapshot, snapshot);
        assert_eq!(tick.snapshot.instrument_token, 144_870_151);
        assert!(tick.snapshot.raw.as_ref().unwrap().full.is_some());
    }

    #[test]
    fn stale_snapshot_is_not_recorded() {
        let (instrument, ts) = crate::paper_flow::simulation::fixture().unwrap();
        let mut snapshot =
            crate::paper_flow::simulation::full_snapshot_for(144_870_151, 6123, ts.as_u64(), 1);
        snapshot.source_fresh = false;
        assert!(collect_snapshot(snapshot, &instrument).unwrap().is_none());
    }
}

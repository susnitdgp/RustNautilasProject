//! Prefix-only historical audit. Completed trades are observations, NOT live entry orders.
use anyhow::{Result, ensure};
use kite_adapter::http::historical::Candle;
use std::collections::BTreeSet;

pub fn run(config: &str, path: &str, date: &str) -> Result<()> {
    let selection = super::ilrc_config::Selection::load(config)?;
    let candles: Vec<Candle> = if path == "--fetch" {
        let date_value = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
        let end = date_value
            .succ_opt()
            .ok_or_else(|| anyhow::anyhow!("date overflow"))?;
        let rt = tokio::runtime::Runtime::new()?;
        let fetched = rt.block_on(kite_adapter::http::historical::fetch_window_for(
            selection.instrument_token,
            end,
            8,
            selection.interval,
        ))?;
        let destination = std::path::PathBuf::from(format!("data/ilrc-test/real-{}.json", date));
        std::fs::create_dir_all(destination.parent().expect("parent"))?;
        std::fs::write(&destination, serde_json::to_vec_pretty(&fetched)?)?;
        println!(
            "{}",
            serde_json::json!({"event":"ilrc_historical_fixture_saved","file":destination,"bars":fetched.len()})
        );
        fetched
    } else {
        serde_json::from_str(&std::fs::read_to_string(path)?)?
    };
    ensure!(
        candles.len() >= 105,
        "at least 105 ordered 3-minute bars required"
    );
    for pair in candles.windows(2) {
        ensure!(
            pair[0].timestamp < pair[1].timestamp,
            "candles must be strictly ordered and unique"
        );
        ensure!(
            pair[1].time()?.timestamp() - pair[0].time()?.timestamp() >= 180,
            "fixture bars must not overlap"
        );
    }
    let a_events = super::ilrc_backtest::entry_events_config_candles(&selection, &candles)?;
    let b_events = super::ilrc_continuation_backtest::entry_events_candles(
        &candles,
        selection.continuation.target_r,
    )?;
    let mut events = a_events.into_iter().chain(b_events).collect::<Vec<_>>();
    events.sort_by(|a, b| a.observed_at.cmp(&b.observed_at).then(a.setup.cmp(b.setup)));
    for event in &events {
        let signal_time = chrono::DateTime::parse_from_rfc3339(&event.entry_time)?;
        let observed = chrono::DateTime::parse_from_rfc3339(&event.observed_at)?;
        ensure!(
            observed >= signal_time + chrono::Duration::minutes(3),
            "entry becomes available before bar completion"
        );
    }
    // Recompute each emitted decision from the prefix ending with its entry candle.
    // A decision cannot be released until that candle is complete.
    let mut verified_prefix_entries = 0usize;
    for event in &events {
        let event_ts = chrono::DateTime::parse_from_rfc3339(&event.entry_time)?.timestamp();
        let Some(idx) = candles
            .iter()
            .position(|c| c.time().is_ok_and(|t| t.timestamp() == event_ts))
        else {
            anyhow::bail!("entry refers to a bar absent from the historical feed");
        };
        let prefix = &candles[..=idx];
        let observed = if event.setup == "A" {
            super::ilrc_backtest::entry_events_config_candles(&selection, prefix)?
        } else {
            super::ilrc_continuation_backtest::entry_events_candles(
                prefix,
                selection.continuation.target_r,
            )?
        };
        ensure!(
            observed.iter().any(|p| p.entry_time == event.entry_time
                && p.side == event.side
                && p.entry == event.entry
                && p.stop == event.stop
                && p.target == event.target),
            "entry decision changed when future candles were withheld"
        );
        verified_prefix_entries += 1;
    }
    println!(
        "{}",
        serde_json::json!({"event":"ilrc_entry_decision_audit","candidate_entries":events.len(),"prefix_verified_entries":verified_prefix_entries,"setup_a":events.iter().filter(|e|e.setup=="A").count(),"setup_b":events.iter().filter(|e|e.setup=="B").count(),"observed_after_close":true,"broker_orders_sent":false,"execution_client_loaded":false,"orders_submitted":0})
    );
    let full = super::ilrc_backtest::evaluate_combined_config_candles(&selection, &candles, date)?;
    let mut confirmed = BTreeSet::<String>::new();
    let mut observed = Vec::<String>::new();
    let mut changes = 0usize;
    for end in 105..=candles.len() {
        let prefix = &candles[..end];
        let trades =
            super::ilrc_backtest::evaluate_combined_config_candles(&selection, prefix, date)?;
        let current = prefix.last().expect("nonempty").time()?.timestamp();
        for t in trades {
            // The evaluated bar can close only after its 3-minute interval completes.
            if chrono::DateTime::parse_from_rfc3339(&t.exit_time)?.timestamp() > current {
                continue;
            }
            let key = format!(
                "{}|{}|{}|{}|{}|{}",
                t.entry_time, t.exit_time, t.side, t.entry, t.exit, t.reason
            );
            if confirmed.insert(key.clone()) {
                observed.push(key);
            }
        }
    }
    let full_keys: BTreeSet<String> = full
        .iter()
        .map(|t| {
            format!(
                "{}|{}|{}|{}|{}|{}",
                t.entry_time, t.exit_time, t.side, t.entry, t.exit, t.reason
            )
        })
        .collect();
    for key in &observed {
        if !full_keys.contains(key) {
            changes += 1;
        }
    }
    println!(
        "{}",
        serde_json::json!({"event":"ilrc_prefix_causality_audit","bars":candles.len(),"batch_closed_trades":full.len(),"prefix_observed_closed_trades":observed.len(),"prefix_results_absent_from_full":changes,"live_signals_implemented":false,"broker_orders_sent":false,"execution_client_loaded":false})
    );
    ensure!(
        changes == 0,
        "historical prefix results changed after later candles; not safe for order generation"
    );
    Ok(())
}

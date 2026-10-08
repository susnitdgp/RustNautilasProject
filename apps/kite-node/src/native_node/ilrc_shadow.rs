//! Production-data shadow runner for ILRC v1.
//! Deliberately contains no execution client or order submission path.
use anyhow::{Result, ensure};
use std::{collections::BTreeSet, time::Duration};

pub fn run(config: &str, seconds: u64) -> Result<()> {
    ensure!(
        (5..=86_360).contains(&seconds),
        "Shadow duration must be 5..86360 seconds"
    );
    let selection = super::ilrc_config::Selection::load(config)?;
    ensure!(
        !selection.live_orders_enabled,
        "ILRC shadow requires live_orders_enabled=false"
    );

    let id = nautilus_core::UUID4::new();
    let folder = std::path::PathBuf::from(format!("data/ilrc-shadow/{id}"));
    std::fs::create_dir_all(&folder)?;

    println!(
        "{}",
        serde_json::json!({
            "event":"ilrc_shadow_started",
            "namespace":id.to_string(),
            "strategy":selection.strategy,
            "instrument":selection.instrument,
            "instrument_token":selection.instrument_token,
            "interval":selection.interval.as_str(),
            "live_orders_enabled":false,
            "execution_client_loaded":false
        })
    );

    let start = std::time::Instant::now();
    let mut seen = BTreeSet::<String>::new();
    let mut emitted = Vec::<serde_json::Value>::new();
    let mut last_bar = None::<String>;
    let rt = tokio::runtime::Runtime::new()?;

    while start.elapsed().as_secs() < seconds {
        let now =
            chrono::Utc::now().with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"));
        let date = now.date_naive();

        if selection.session_calendar.session(date)?.is_none() {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }

        let raw = rt.block_on(kite_adapter::http::historical::fetch_window_for(
            selection.instrument_token,
            date,
            8,
            selection.interval,
        ))?;

        if let Some(c) = raw.last() {
            last_bar = Some(c.timestamp.clone());
        }

        let date_str = date.to_string();
        let report = super::ilrc_backtest::evaluate_config_candles(&selection, &raw, &date_str)?;
        super::ilrc_dashboard::render(super::ilrc_dashboard::Snapshot {
            namespace: &id.to_string(),
            strategy: &selection.strategy,
            instrument: &selection.instrument,
            interval: selection.interval.as_str(),
            now,
            session_open_minute: selection.session_open_minute,
            entry_cutoff_minute: selection.entry_cutoff_minute,
            last_bar: raw.last(),
            bars_loaded: raw.len(),
            trades: &report,
            report_directory: &folder,
        });
        for trade in report {
            let key = format!(
                "{}|{}|{}|{}",
                trade.entry_time, trade.exit_time, trade.side, trade.entry
            );
            if seen.insert(key) {
                let event = serde_json::json!({
                    "event":"ilrc_shadow_trade",
                    "namespace":id.to_string(),
                    "trade":trade,
                    "live_orders_enabled":false,
                    "broker_orders_sent":false
                });
                println!("{event}");
                emitted.push(event);
            }
        }

        std::thread::sleep(Duration::from_secs(15));
    }

    let output = serde_json::json!({
        "event":"ilrc_shadow_complete",
        "namespace":id.to_string(),
        "strategy":selection.strategy,
        "instrument":selection.instrument,
        "interval":selection.interval.as_str(),
        "last_bar":last_bar,
        "shadow_events":emitted.len(),
        "live_orders_enabled":false,
        "execution_client_loaded":false,
        "broker_orders_sent":false,
        "report_directory":folder
    });
    super::backtest_report::json(&folder, "events.json", &emitted)?;
    super::backtest_report::json(&folder, "summary.json", &output)?;
    println!("{output}");
    Ok(())
}

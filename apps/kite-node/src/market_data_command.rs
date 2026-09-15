use crate::preflight_command::{read_config, resolve};
use anyhow::{Result, ensure};
use kite_adapter::{
    auth::session,
    credentials::redis,
    websocket::supervisor::{self, FeedEvent},
};
use std::time::Duration;

pub fn session_check(path: &str) -> Result<()> {
    let config = read_config(path)?;
    let credentials = redis::load_from_env()?;
    tokio::runtime::Runtime::new()?.block_on(session::validate(&credentials, &config.exchange))?;
    println!(
        "{}",
        serde_json::json!({
            "kite_session_validated": true, "exchange_enabled": config.exchange,
            "live_orders_enabled": false,
        })
    );
    Ok(())
}

pub fn stream(path: &str, seconds: u64) -> Result<()> {
    let config = read_config(path)?;
    let credentials = redis::load_from_env()?;
    // Blocking public master read stays outside Tokio and the eventual core loop.
    let report = resolve(&config, None)?;
    tokio::runtime::Runtime::new()?.block_on(async {
        session::validate(&credentials, &config.exchange).await?;
        println!(
            "{}",
            serde_json::json!({
                "event": "session_validated", "instrument_id": report.instrument_id,
                "seconds": seconds, "live_orders_enabled": false,
            })
        );
        let mut samples = 0;
        let run = supervisor::observe(
            &credentials,
            report.instrument_token,
            Duration::from_secs(seconds),
            |event| {
                if matches!(event, FeedEvent::Snapshot(_)) {
                    if samples >= 5 {
                        return;
                    }
                    samples += 1;
                }
                // Only typed, non-secret events can reach this output.
                if let Ok(line) = serde_json::to_string(&event) {
                    println!("{line}");
                }
            },
        );
        let summary = tokio::select! {
            result = run => result?,
            _ = tokio::signal::ctrl_c() => {
                return Err(anyhow::anyhow!("Stream interrupted; connection dropped"));
            }
        };
        println!("{}", serde_json::json!({"event":"summary", "data":summary}));
        ensure!(
            summary.full_ticks > 0,
            "No full-mode ticks received; market data verification incomplete"
        );
        ensure!(
            summary.final_source_fresh,
            "Final market snapshot is stale; verification incomplete"
        );
        Ok(())
    })
}

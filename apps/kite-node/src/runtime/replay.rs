use super::core::Core;
use anyhow::{Result, ensure};
use kite_recorder::{records::Record, replay};
use std::path::Path;

/// Quote transport replay only; no timers, strategies, execution or broker access.
pub fn run(path: &Path) -> Result<()> {
    // Validate completion before applying a potentially incomplete capture.
    let expected = replay::replay(path, |_| Ok(()))?;
    let mut core = None;
    let result = replay::replay(path, |record| {
        match record {
            Record::Header { instrument, .. } => core = Some(Core::new(&instrument)),
            Record::Quote { quote, .. } => core.as_mut().expect("validated header").quote(quote),
            _ => {}
        }
        Ok(())
    })?;
    let core = core.expect("validated header");
    ensure!(
        result.quotes == expected.quotes && core.callbacks() == expected.quotes,
        "Replay callback mismatch"
    );
    println!(
        "{}",
        serde_json::json!({
            "event":"replay_complete", "quotes":result.quotes, "records":result.records,
            "gaps":result.gaps, "data_engine_callbacks":core.callbacks(),
            "broker_accessed":false, "live_orders_enabled":false,
        })
    );
    Ok(())
}

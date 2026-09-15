use crate::preflight_command::{read_config, resolve};
use anyhow::{Result, ensure};
use kite_adapter::{credentials::redis, reconciliation};
pub fn run(path: &str) -> Result<()> {
    let config = read_config(path)?;
    let credentials = redis::load_from_env()?;
    let report = resolve(&config, None)?;
    let summary = tokio::runtime::Runtime::new()?.block_on(async {
        tokio::select! {
            result = reconciliation::service::run(&credentials, &config.expected_symbol, report.instrument_token) => result,
            _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("Reconciliation interrupted")),
        }
    })?;
    println!("{}", serde_json::to_string(&summary)?);
    ensure!(
        summary.consistency_checks_passed,
        "Read-only reconciliation needs review; see issue codes"
    );
    Ok(())
}

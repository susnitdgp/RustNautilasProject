use anyhow::{Context, Result, ensure};
use chrono::{FixedOffset, Utc};
use kite_adapter::{config::Config, credentials::redis, http::instruments, preflight};
use std::{fs, io::Read};

pub fn read_config(path: &str) -> Result<Config> {
    Config::parse(&fs::read_to_string(path).context("Read configuration")?)
}

pub fn resolve(config: &Config, csv: Option<&str>) -> Result<preflight::Report> {
    let bytes = if let Some(path) = csv {
        let file = fs::File::open(path).context("Open instrument CSV")?;
        let mut bytes = Vec::new();
        file.take(instruments::MAX_MASTER_BYTES + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= instruments::MAX_MASTER_BYTES,
            "CSV exceeds size limit"
        );
        bytes
    } else {
        instruments::download()?
    };
    let india = FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid IST offset");
    preflight::run(
        config,
        bytes.as_slice(),
        Utc::now().with_timezone(&india).date_naive(),
    )
}

pub fn run(path: &str, csv: Option<&str>) -> Result<()> {
    let config = read_config(path)?;
    let _credentials = redis::load_from_env()?;
    let report = resolve(&config, csv)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "source": if csv.is_some() { "user_supplied_csv_freshness_unverified" } else { "kite_live_download" },
            "checked_at_utc": Utc::now(), "report": report,
            "credentials_loaded": true, "kite_session_validated": false,
        }))?
    );
    Ok(())
}

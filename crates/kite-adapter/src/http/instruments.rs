use anyhow::{Context, Result, ensure};
use std::{io::Read, time::Duration};

pub const INSTRUMENTS_URL: &str = "https://api.kite.trade/instruments/MCX";
pub const MAX_MASTER_BYTES: u64 = 32 * 1024 * 1024;

pub fn download() -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .get(INSTRUMENTS_URL)
        .send()
        .context("Could not download MCX instrument master")?
        .error_for_status()
        .context("Kite instruments HTTP failure")?;
    let mut bytes = Vec::new();
    response
        .take(MAX_MASTER_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_MASTER_BYTES,
        "Instrument master exceeds size limit"
    );
    ensure!(!bytes.is_empty(), "Empty instrument response");
    Ok(bytes)
}

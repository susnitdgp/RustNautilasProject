use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::io::Read;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InstrumentRow {
    pub instrument_token: u32,
    pub tradingsymbol: String,
    pub name: String,
    pub expiry: String,
    pub tick_size: String,
    pub lot_size: u32,
    pub instrument_type: String,
    pub segment: String,
    pub exchange: String,
}

pub fn parse(reader: impl Read) -> Result<Vec<InstrumentRow>> {
    let mut csv = csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(reader);
    let headers = csv.headers().context("Missing CSV header")?;
    for name in [
        "instrument_token",
        "tradingsymbol",
        "name",
        "expiry",
        "tick_size",
        "lot_size",
        "instrument_type",
        "segment",
        "exchange",
    ] {
        ensure!(
            headers.iter().filter(|h| *h == name).count() == 1,
            "Missing or duplicate required CSV column: {name}"
        );
    }
    let rows = csv
        .deserialize()
        .collect::<std::result::Result<Vec<InstrumentRow>, _>>()
        .context("Invalid instrument CSV record")?;
    ensure!(!rows.is_empty(), "Instrument master has no records");
    Ok(rows)
}

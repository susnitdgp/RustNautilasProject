use super::records::Record;
use anyhow::{Result, anyhow, ensure};
use parquet::{
    file::reader::{FileReader, SerializedFileReader},
    record::RowAccessor,
};
use std::{fs::File, path::Path};

#[derive(Debug, serde::Serialize)]
pub struct ReplaySummary {
    pub records: u64,
    pub quotes: u64,
    pub gaps: u64,
}

/// Replays captured order, preserving source and receive timestamps without network access.
pub fn replay(path: &Path, mut consume: impl FnMut(Record) -> Result<()>) -> Result<ReplaySummary> {
    let file = SerializedFileReader::new(File::open(path)?)?;
    let mut instrument = None;
    let mut generation = 0;
    let mut active = false;
    let mut ended = false;
    let mut result = ReplaySummary {
        records: 0,
        quotes: 0,
        gaps: 0,
    };
    for row in file.get_row_iter(None)? {
        ensure!(!ended, "Records after completion marker");
        ensure!(result.records < 1_000_000, "Replay record limit exceeded");
        let row = row?;
        ensure!(
            row.get_long(0)? == i64::try_from(result.records)?,
            "Capture sequence mismatch"
        );
        let payload = row.get_string(2)?;
        ensure!(payload.len() <= 1_048_576, "Replay payload too large");
        let record: Record = serde_json::from_str(payload)?;
        ensure!(row.get_string(1)? == record.kind(), "Capture kind mismatch");
        match &record {
            Record::Header {
                schema_version,
                instrument: value,
                instrument_token,
            } => {
                ensure!(
                    result.records == 0 && *schema_version == 1 && *instrument_token > 0,
                    "Invalid capture header"
                );
                instrument = Some(value.id);
            }
            Record::Connected { generation: value } => {
                ensure!(
                    instrument.is_some() && !active && *value == generation + 1,
                    "Invalid connection generation"
                );
                generation = *value;
                active = true;
            }
            Record::Gap { generation: value } => {
                ensure!(active && *value == generation, "Invalid gap generation");
                active = false;
                result.gaps += 1;
            }
            Record::Quote {
                quote,
                generation: value,
            } => {
                ensure!(
                    active && *value == generation && Some(quote.instrument_id) == instrument,
                    "Quote identity or generation mismatch"
                );
                result.quotes += 1;
            }
            Record::End { quotes } => {
                ensure!(
                    instrument.is_some() && *quotes == result.quotes,
                    "Completion count mismatch"
                );
                ended = true;
            }
        }
        consume(record)?;
        result.records += 1;
    }
    if !ended {
        return Err(anyhow!("Capture is incomplete: no completion marker"));
    }
    Ok(result)
}

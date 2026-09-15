//! Historical market data only. Orders and application state are stored in Redis.
use anyhow::{Result, ensure};
use nautilus_model::{data::QuoteTick, instruments::InstrumentAny};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use std::path::Path;
pub fn write(path: &Path, instrument: InstrumentAny, quotes: &[QuoteTick]) -> Result<()> {
    ensure!(
        !path.exists(),
        "Use a new catalog directory to avoid overlapping history"
    );
    std::fs::create_dir_all(path)?;
    super::full_codec::register()?;
    let catalog = ParquetDataCatalog::new(path, None, None, None, None);
    catalog.write_instruments(vec![instrument])?;
    catalog.write_to_parquet(quotes, None, None, None)?;
    Ok(())
}
pub fn fixture_quotes() -> Result<Vec<QuoteTick>> {
    let (instrument, ts) = crate::paper_flow::simulation::fixture()?;
    [
        6008, 6006, 6004, 6002, 6000, 6002, 6004, 6006, 6005, 6005, 6003, 6003, 6001, 5999, 6000,
    ]
    .iter()
    .enumerate()
    .map(|(i, p)| {
        let snapshot = crate::paper_flow::simulation::full_snapshot(
            *p,
            ts.as_u64() + (i as u64 + 1) * 1_000_000_000,
            1,
        );
        kite_adapter::mapping::quotes::map(&snapshot, &instrument)?
            .ok_or_else(|| anyhow::anyhow!("Fixture quote unavailable"))
    })
    .collect()
}

pub fn write_full(
    path: &Path,
    instrument: InstrumentAny,
    ticks: &[kite_adapter::data::full_tick::KiteFullTick],
) -> Result<()> {
    let quotes: Vec<_> = ticks.iter().map(|t| t.quote).collect();
    write(path, instrument, &quotes)?;
    super::full_codec::register()?;
    let catalog = ParquetDataCatalog::new(path, None, None, None, None);
    let data = ticks
        .iter()
        .map(|t| nautilus_model::data::CustomData::from_arc(std::sync::Arc::new(t.clone())))
        .collect();
    catalog.write_custom_data_batch(data, None, None, None)?;
    ensure!(
        read_full(path)? == ticks,
        "Full packet catalog roundtrip differs from captured data"
    );
    Ok(())
}

/// Read complete packets through Nautilus' registered Arrow decoder.
pub fn read_full(path: &Path) -> Result<Vec<kite_adapter::data::full_tick::KiteFullTick>> {
    super::full_codec::register()?;
    let mut catalog = ParquetDataCatalog::new(path, None, None, None, None);
    catalog
        .query_custom_data_dynamic("KiteFullTick", None, None, None, None, None, true)?
        .into_iter()
        .map(|data| match data {
            nautilus_model::data::Data::Custom(data) => data
                .data
                .as_any()
                .downcast_ref::<kite_adapter::data::full_tick::KiteFullTick>()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Unexpected catalog custom type")),
            _ => Err(anyhow::anyhow!("Unexpected catalog data type")),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_catalog_preserves_complete_packets_and_quote_history() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("catalog");
        let (instrument, ts) = crate::paper_flow::simulation::fixture()?;
        let ticks = [6000, 6002, 6004]
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let snapshot = crate::paper_flow::simulation::full_snapshot(
                    p,
                    ts.as_u64() + (i as u64 + 1) * 1_000_000_000,
                    1,
                );
                let quote = kite_adapter::mapping::quotes::map(&snapshot, &instrument)?.unwrap();
                Ok(kite_adapter::data::full_tick::KiteFullTick { snapshot, quote })
            })
            .collect::<Result<Vec<_>>>()?;
        write_full(&path, InstrumentAny::FuturesContract(instrument), &ticks)?;
        assert_eq!(read_full(&path)?, ticks);
        let mut catalog = ParquetDataCatalog::new(&path, None, None, None, None);
        assert_eq!(
            catalog.quote_ticks(None, None, None)?,
            ticks.iter().map(|t| t.quote).collect::<Vec<_>>()
        );
        Ok(())
    }
}

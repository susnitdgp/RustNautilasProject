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

/// Audit complete decoded fields without exposing raw market/account payloads.
pub fn audit(path: &Path) -> Result<()> {
    let ticks = read_full(path)?;
    ensure!(!ticks.is_empty(), "No full packets in catalog");
    let mut timestamped = 0;
    let mut populated_depth = 0;
    for tick in &ticks {
        let raw = tick
            .snapshot
            .raw
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Raw tick missing"))?;
        let full = raw
            .full
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Full fields missing"))?;
        ensure!(
            raw.quote_fields.is_some()
                && raw.last_quantity.is_some()
                && raw.cumulative_volume.is_some(),
            "Quote/trade snapshot fields missing"
        );
        ensure!(
            raw.instrument_token == tick.snapshot.instrument_token,
            "Tick identity mismatch"
        );
        if full.last_trade_timestamp.is_some() && full.exchange_timestamp.is_some() {
            timestamped += 1;
        }
        if full.bids.iter().all(|l| l.quantity > 0) && full.asks.iter().all(|l| l.quantity > 0) {
            populated_depth += 1;
        }
    }
    println!(
        "{}",
        serde_json::json!({"event":"native_full_tick_audit","catalog":path,"packets":ticks.len(),"full_fields_present":true,"bid_levels":5,"ask_levels":5,"packets_with_timestamps":timestamped,"packets_with_all_depth_levels_populated":populated_depth,"available_fields":["ltp","last_quantity","cumulative_volume","average_price","total_buy_quantity","total_sell_quantity","OHLC","OI","OI_day_high","OI_day_low","last_trade_timestamp","exchange_timestamp","depth_price_quantity_order_count","receive_time","connection_generation"],"unique_trade_ids":false,"aggressor_side":false,"exchange_order_deltas":false,"live_orders_enabled":false})
    );
    Ok(())
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
                let mut snapshot = crate::paper_flow::simulation::full_snapshot(
                    p,
                    ts.as_u64() + (i as u64 + 1) * 1_000_000_000,
                    1,
                );
                let raw = snapshot.raw.as_mut().unwrap();
                let full = raw.full.as_mut().unwrap();
                full.open_interest_day_high = 98765;
                full.open_interest_day_low = 12345;
                for (j, level) in full.bids.iter_mut().enumerate() {
                    level.orders = 10 + j as u16;
                    level.quantity = 100 + j as u32;
                }
                for (j, level) in full.asks.iter_mut().enumerate() {
                    level.orders = 20 + j as u16;
                    level.quantity = 200 + j as u32;
                }
                raw.quote_fields.as_mut().unwrap().total_buy_quantity = 34567;
                raw.quote_fields.as_mut().unwrap().total_sell_quantity = 45678;
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

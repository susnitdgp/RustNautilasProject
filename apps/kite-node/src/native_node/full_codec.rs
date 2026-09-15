//! Native Arrow registry codec: complete Kite packet JSON plus indexed native timestamps.
use anyhow::{Result, anyhow, ensure};
use arrow::{
    array::{Array, ArrayRef, StringArray, UInt64Array},
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use kite_adapter::data::full_tick::KiteFullTick;
use nautilus_model::data::{
    CustomData, CustomDataTrait, Data, HasTsInit, custom::ensure_custom_data_json_registered,
    registry::ensure_arrow_registered,
};
use std::sync::Arc;
pub fn register() -> Result<()> {
    ensure_custom_data_json_registered::<KiteFullTick>()?;
    let schema = Arc::new(Schema::new(vec![
        Field::new("ts_event", DataType::UInt64, false),
        Field::new("ts_init", DataType::UInt64, false),
        Field::new("payload", DataType::Utf8, false),
    ]));
    let enc_schema = schema.clone();
    ensure_arrow_registered(
        "KiteFullTick",
        schema,
        Box::new(move |items| {
            let ticks: Vec<_> = items
                .iter()
                .map(|item| {
                    item.as_any()
                        .downcast_ref::<KiteFullTick>()
                        .ok_or_else(|| anyhow!("Wrong full tick type"))
                })
                .collect::<Result<_>>()?;
            let payload: Vec<String> = ticks.iter().map(|t| t.to_json()).collect::<Result<_>>()?;
            let arrays: Vec<ArrayRef> = vec![
                Arc::new(UInt64Array::from(
                    ticks
                        .iter()
                        .map(|t| t.ts_event().as_u64())
                        .collect::<Vec<_>>(),
                )),
                Arc::new(UInt64Array::from(
                    ticks
                        .iter()
                        .map(|t| t.ts_init().as_u64())
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(payload)),
            ];
            Ok(RecordBatch::try_new(enc_schema.clone(), arrays)?)
        }),
        Box::new(|_, batch| {
            // DataFusion may return Utf8View instead of the written Utf8 representation.
            let payload_column = batch
                .column_by_name("payload")
                .ok_or_else(|| anyhow!("Missing full tick payload"))?;
            let normalized = arrow::compute::cast(payload_column, &DataType::Utf8)?;
            let payload = normalized
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow!("Invalid full tick payload type"))?;
            let times = batch
                .column_by_name("ts_init")
                .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
                .ok_or_else(|| anyhow!("Missing full tick time"))?;
            let event_times = batch
                .column_by_name("ts_event")
                .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
                .ok_or_else(|| anyhow!("Missing full tick event time"))?;
            (0..batch.num_rows())
                .map(|i| {
                    ensure!(
                        !payload.is_null(i) && !times.is_null(i) && !event_times.is_null(i),
                        "Null full tick field"
                    );
                    let tick: KiteFullTick = serde_json::from_str(payload.value(i))?;
                    ensure!(
                        tick.ts_init().as_u64() == times.value(i)
                            && tick.ts_event().as_u64() == event_times.value(i),
                        "Full tick catalog timestamp mismatch"
                    );
                    Ok(Data::Custom(CustomData::from_arc(Arc::new(tick))))
                })
                .collect()
        }),
    )?;
    Ok(())
}

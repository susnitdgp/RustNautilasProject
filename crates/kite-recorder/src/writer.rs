use super::records::Record;
use anyhow::{Result, anyhow, ensure};
use parquet::{
    basic::Compression,
    data_type::{ByteArray, ByteArrayType, Int64Type},
    file::{properties::WriterProperties, writer::SerializedFileWriter},
    schema::parser::parse_message_type,
};
use std::{
    fs::{File, OpenOptions},
    path::Path,
    sync::{
        Arc,
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
};

pub struct Recorder {
    sender: Option<SyncSender<Record>>,
    worker: Option<JoinHandle<Result<u64>>>,
}
impl Recorder {
    pub fn create(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let (sender, receiver) = mpsc::sync_channel(1024);
        let worker = thread::spawn(move || {
            let sync_file = file.try_clone()?;
            let schema = Arc::new(parse_message_type(
                "message kite_capture { REQUIRED INT64 sequence; REQUIRED BINARY kind (UTF8); REQUIRED BINARY payload (UTF8); }",
            )?);
            let properties = Arc::new(
                WriterProperties::builder()
                    .set_compression(Compression::SNAPPY)
                    .build(),
            );
            let mut writer = SerializedFileWriter::new(file, schema, properties)?;
            let mut batch = Vec::new();
            let mut sequence = 0u64;
            for record in receiver {
                batch.push(record);
                if batch.len() == 256 {
                    write_batch(&mut writer, &batch, sequence)?;
                    sequence += batch.len() as u64;
                    batch.clear();
                }
            }
            if !batch.is_empty() {
                write_batch(&mut writer, &batch, sequence)?;
                sequence += batch.len() as u64;
            }
            writer.close()?;
            sync_file.sync_all()?;
            Ok(sequence)
        });
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }
    /// A full/failed recorder queue aborts the run; observations are never silently dropped.
    pub fn record(&self, record: Record) -> Result<()> {
        self.sender
            .as_ref()
            .ok_or_else(|| anyhow!("Recorder closed"))?
            .try_send(record)
            .map_err(|_| anyhow!("Recorder queue full or writer failed"))
    }
    pub fn finish(mut self) -> Result<u64> {
        self.sender.take();
        self.worker
            .take()
            .expect("writer handle")
            .join()
            .map_err(|_| anyhow!("Recorder worker panicked"))?
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn write_batch(
    writer: &mut SerializedFileWriter<File>,
    records: &[Record],
    first: u64,
) -> Result<()> {
    let sequence = (0..records.len())
        .map(|i| i64::try_from(first + i as u64))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let kinds = records
        .iter()
        .map(|r| ByteArray::from(r.kind()))
        .collect::<Vec<_>>();
    let payloads = records
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        payloads.iter().all(|p| p.len() <= 1_048_576),
        "Recorder payload too large"
    );
    let payloads = payloads
        .iter()
        .map(|p| ByteArray::from(p.as_str()))
        .collect::<Vec<_>>();
    let mut group = writer.next_row_group()?;
    let mut column = group
        .next_column()?
        .ok_or_else(|| anyhow!("Missing sequence column"))?;
    column
        .typed::<Int64Type>()
        .write_batch(&sequence, None, None)?;
    column.close()?;
    let mut column = group
        .next_column()?
        .ok_or_else(|| anyhow!("Missing kind column"))?;
    column
        .typed::<ByteArrayType>()
        .write_batch(&kinds, None, None)?;
    column.close()?;
    let mut column = group
        .next_column()?
        .ok_or_else(|| anyhow!("Missing payload column"))?;
    column
        .typed::<ByteArrayType>()
        .write_batch(&payloads, None, None)?;
    column.close()?;
    group.close()?;
    Ok(())
}

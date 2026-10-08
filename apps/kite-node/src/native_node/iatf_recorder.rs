//! Manual, read-only CRUDEOILM full-depth recorder for offline IATF replay.
//! No execution client, order submission, or Redis writes.
use anyhow::{Context, Result, ensure};
use kite_adapter::{
    credentials::redis,
    http::instruments,
    instruments::master,
    mapping::market_data::Snapshot,
    websocket::{
        supervisor::{self, FeedEvent},
        transport,
    },
};
use serde::Deserialize;
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    time::Duration,
};
use tokio::time::Instant;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    symbol: String,
    instrument_token: u32,
    expected_expiry: String,
    seconds: u64,
    output: String,
    live_orders_enabled: bool,
}
fn validate(s: &Settings) -> Result<()> {
    ensure!(
        !s.live_orders_enabled,
        "Recorder may not enable live orders"
    );
    ensure!(
        s.symbol == "CRUDEOILM26OCTFUT" && s.expected_expiry == "2026-10-19",
        "Unexpected CRUDEOILM contract"
    );
    ensure!(
        s.instrument_token > 0 && (10..=7200).contains(&s.seconds),
        "Invalid token/duration"
    );
    ensure!(
        s.output.starts_with("data/iatf-recordings/")
            && s.output.ends_with(".jsonl")
            && !s.output.contains(".."),
        "Output must be under data/iatf-recordings/"
    );
    Ok(())
}
fn record(s: &Snapshot) -> Result<Option<String>> {
    if !s.source_fresh {
        return Ok(None);
    }
    let (Some(exchange), Some(bid), Some(ask), Some(bid_qty), Some(ask_qty)) = (
        s.exchange_timestamp,
        s.bid.as_deref(),
        s.ask.as_deref(),
        s.bid_size,
        s.ask_size,
    ) else {
        return Ok(None);
    };
    if bid_qty == 0 || ask_qty == 0 {
        return Ok(None);
    }
    let received = s.received_at_utc.timestamp_millis();
    ensure!(received >= 0, "Invalid receive timestamp");
    let v = serde_json::json!({
        "generation":s.connection_generation,
        "instrument_token":s.instrument_token,
        "exchange_ts_ms":u64::from(exchange)*1000,
        "received_ts_ms":received as u64,
        "bid":bid.parse::<f64>()?,"ask":ask.parse::<f64>()?,
        "bid_qty":bid_qty,"ask_qty":ask_qty,"last":s.ltp.parse::<f64>()?
    });
    Ok(Some(serde_json::to_string(&v)?))
}
pub fn run(path: &str) -> Result<()> {
    let s: Settings = serde_json::from_slice(&std::fs::read(path)?)?;
    validate(&s)?;
    // Public broker master is consulted before loading credentials or opening a WebSocket.
    let master = master::parse(instruments::download()?.as_slice())?;
    let matched: Vec<_> = master
        .iter()
        .filter(|row| {
            row.tradingsymbol == s.symbol
                && row.instrument_type == "FUT"
                && row.name == "CRUDEOILM"
                && row.exchange == "MCX"
                && row.segment == "MCX-FUT"
                && row.expiry == s.expected_expiry
                && row.instrument_token == s.instrument_token
        })
        .collect();
    ensure!(
        matched.len() == 1,
        "Contract/token/expiry did not match Kite instrument master"
    );
    std::fs::create_dir_all("data/iatf-recordings")?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&s.output)
        .context("Recorder output already exists; never overwrite tick data")?;
    let mut writer = BufWriter::new(file);
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let creds = redis::load_from_env()?;
        let socket = transport::connect(&creds, Instant::now() + Duration::from_secs(10)).await?;
        let mut count = 0usize;
        let mut failed = None::<String>;
        let mut previous = None::<(u32, u64, u64)>;
        let result = supervisor::observe_connected(
            &creds,
            s.instrument_token,
            Duration::from_secs(s.seconds),
            |event| {
                if failed.is_some() {
                    return;
                }
                match event {
                    FeedEvent::Connected { generation } => {
                        if generation != 1 {
                            failed = Some("WebSocket reconnected; recording discontinued".into());
                        }
                    }
                    FeedEvent::Gap { .. } => {
                        failed = Some("WebSocket gap; recording discontinued".into())
                    }
                    FeedEvent::Snapshot(snapshot) => match record(&snapshot) {
                        Ok(Some(line)) => {
                            let time = (
                                snapshot.connection_generation,
                                u64::from(snapshot.exchange_timestamp.unwrap_or_default()) * 1000,
                                snapshot.received_at_utc.timestamp_millis() as u64,
                            );
                            if previous.is_some_and(|p| time <= p) {
                                failed = Some("Repeated/non-monotonic recorded timestamp".into());
                            } else if let Err(error) = writeln!(writer, "{line}") {
                                failed = Some(format!("Recording write failed: {error}"));
                            } else {
                                previous = Some(time);
                                count += 1;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => failed = Some(error.to_string()),
                    },
                }
            },
            socket,
        )
        .await;
        writer.flush()?;
        result?;
        ensure!(failed.is_none(), "Unusable recording: {:?}", failed);
        ensure!(count > 0, "No valid full-depth snapshots recorded");
        println!(
            "{}",
            serde_json::json!({"event":"iatf_recording_complete",
            "records":count,"output":s.output,"live_orders_enabled":false,"orders_sent":0})
        );
        Ok(())
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_live_and_unsafe_path() {
        let mut s: Settings = serde_json::from_str(include_str!(
            "../../../../config/iatf-recorder.example.json"
        ))
        .unwrap();
        validate(&s).unwrap();
        s.live_orders_enabled = true;
        assert!(validate(&s).is_err());
        s.live_orders_enabled = false;
        s.output = "../oops.jsonl".into();
        assert!(validate(&s).is_err());
    }
}

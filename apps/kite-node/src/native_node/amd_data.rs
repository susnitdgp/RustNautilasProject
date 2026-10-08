//! Historical candles only for independent AMD Power of Three P&L evaluation.
use anyhow::{Result, ensure};
use chrono::NaiveDate;
use kite_adapter::http::historical::{self, Interval};
pub fn export(token: u32, minutes: u32) -> Result<()> {
    ensure!(token > 0, "Invalid token");
    ensure!(
        minutes == 3 || minutes == 5,
        "Only 3m and 5m export supported"
    );
    let interval = if minutes == 3 {
        Interval::ThreeMinute
    } else {
        Interval::FiveMinute
    };
    let rt = tokio::runtime::Runtime::new()?;
    let candles = rt.block_on(async {
        let mut reader = historical::Reader::default();
        let mut all = Vec::new();
        for (date, days) in [("2026-08-25", 24), ("2026-09-17", 23), ("2026-10-09", 22)] {
            all.extend(
                reader
                    .fetch_window_for(
                        token,
                        NaiveDate::parse_from_str(date, "%Y-%m-%d")?,
                        days,
                        interval,
                    )
                    .await?,
            );
        }
        Ok::<_, anyhow::Error>(all)
    })?;
    let mut candles = candles;
    candles.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    candles.dedup_by(|a, b| a.timestamp == b.timestamp);
    historical::validate_for(&candles, interval)?;
    println!("{}", serde_json::to_string(&candles)?);
    Ok(())
}

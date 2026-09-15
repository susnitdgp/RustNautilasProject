use crate::mapping::market_data::Snapshot;
use anyhow::{Result, ensure};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::QuoteTick,
    instruments::FuturesContract,
    types::{Price, Quantity},
};
use rust_decimal::Decimal;
use std::str::FromStr;

/// Missing, stale or one-sided observations remain diagnostic snapshots, not quotes.
pub fn map(snapshot: &Snapshot, instrument: &FuturesContract) -> Result<Option<QuoteTick>> {
    if !snapshot.source_fresh {
        return Ok(None);
    }
    let (Some(bid), Some(ask), Some(bid_size), Some(ask_size), Some(source)) = (
        &snapshot.bid,
        &snapshot.ask,
        snapshot.bid_size,
        snapshot.ask_size,
        snapshot.exchange_timestamp,
    ) else {
        return Ok(None);
    };
    if bid_size == 0 || ask_size == 0 {
        return Ok(None);
    }
    let bid = Decimal::from_str(bid)?;
    let ask = Decimal::from_str(ask)?;
    ensure!(bid <= ask, "Crossed quote rejected");
    ensure!(
        bid.fract().is_zero() && ask.fract().is_zero(),
        "Quote violates the verified MCX tick size"
    );
    let init: UnixNanos = u64::try_from(
        snapshot
            .received_at_utc
            .timestamp_nanos_opt()
            .ok_or_else(|| anyhow::anyhow!("Receive timestamp overflow"))?,
    )?
    .into();
    Ok(Some(QuoteTick::new_checked(
        instrument.id,
        Price::from_str(&bid.normalize().to_string()).map_err(anyhow::Error::msg)?,
        Price::from_str(&ask.normalize().to_string()).map_err(anyhow::Error::msg)?,
        Quantity::from(bid_size),
        Quantity::from(ask_size),
        (u64::from(source) * 1_000_000_000).into(),
        init,
    )?))
}

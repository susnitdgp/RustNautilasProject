//! Parses documented Kite 8/44/184-byte packets for the MCX subscription.
use super::models::{BinaryFrame, DepthLevel, FullFields, Tick};
use anyhow::{Result, ensure};

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated packet"),
    )
}
fn i32_at(bytes: &[u8], offset: usize) -> i32 {
    i32::from_be_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated packet"),
    )
}
fn timestamp(value: u32) -> Option<u32> {
    (value != 0).then_some(value)
}

fn parse_packet(bytes: &[u8]) -> Result<Tick> {
    ensure!(
        matches!(bytes.len(), 8 | 44 | 184),
        "Unsupported MCX packet length"
    );
    let mut tick = Tick {
        instrument_token: u32_at(bytes, 0),
        ltp_paise: i32_at(bytes, 4),
        last_quantity: None,
        cumulative_volume: None,
        full: None,
    };
    ensure!(
        tick.instrument_token != 0,
        "Zero WebSocket instrument token"
    );
    if bytes.len() >= 44 {
        tick.last_quantity = Some(u32_at(bytes, 8));
        tick.cumulative_volume = Some(u32_at(bytes, 16));
    }
    if bytes.len() == 184 {
        let depth: [DepthLevel; 10] = std::array::from_fn(|index| {
            let offset = 64 + index * 12;
            DepthLevel {
                quantity: u32_at(bytes, offset),
                price_paise: i32_at(bytes, offset + 4),
                orders: u16::from_be_bytes(
                    bytes[offset + 8..offset + 10]
                        .try_into()
                        .expect("validated depth"),
                ),
            }
        });
        tick.full = Some(FullFields {
            last_trade_timestamp: timestamp(u32_at(bytes, 44)),
            exchange_timestamp: timestamp(u32_at(bytes, 60)),
            open_interest: u32_at(bytes, 48),
            bids: depth[..5].try_into().expect("five bids"),
            asks: depth[5..].try_into().expect("five asks"),
        });
    }
    Ok(tick)
}

pub fn parse(bytes: &[u8]) -> Result<BinaryFrame> {
    if bytes.len() == 1 {
        return Ok(BinaryFrame::Heartbeat);
    }
    ensure!(
        bytes.len() >= 2 && bytes.len() <= 1_048_576,
        "Invalid binary frame size"
    );
    let count = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    ensure!(count <= 4096, "Too many packets in frame");
    let mut cursor = 2;
    let mut ticks = Vec::with_capacity(count);
    for _ in 0..count {
        ensure!(cursor + 2 <= bytes.len(), "Truncated packet length");
        let len = u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]) as usize;
        cursor += 2;
        ensure!(cursor + len <= bytes.len(), "Truncated packet body");
        ticks.push(parse_packet(&bytes[cursor..cursor + len])?);
        cursor += len;
    }
    ensure!(cursor == bytes.len(), "Unexpected trailing frame bytes");
    Ok(BinaryFrame::Ticks(ticks))
}

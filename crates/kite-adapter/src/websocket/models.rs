use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct DepthLevel {
    pub quantity: u32,
    pub price_paise: i32,
    pub orders: u16,
}
#[derive(Debug, Clone, Serialize)]
pub struct FullFields {
    pub last_trade_timestamp: Option<u32>,
    pub exchange_timestamp: Option<u32>,
    pub open_interest: u32,
    pub bids: [DepthLevel; 5],
    pub asks: [DepthLevel; 5],
}
#[derive(Debug, Clone, Serialize)]
pub struct Tick {
    pub instrument_token: u32,
    pub ltp_paise: i32,
    pub last_quantity: Option<u32>,
    pub cumulative_volume: Option<u32>,
    pub full: Option<FullFields>,
}
#[derive(Debug)]
pub enum BinaryFrame {
    Heartbeat,
    Ticks(Vec<Tick>),
}

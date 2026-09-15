use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct DepthLevel {
    pub quantity: u32,
    pub price_paise: i32,
    pub orders: u16,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FullFields {
    pub open_interest_day_high: u32,
    pub open_interest_day_low: u32,
    pub last_trade_timestamp: Option<u32>,
    pub exchange_timestamp: Option<u32>,
    pub open_interest: u32,
    pub bids: [DepthLevel; 5],
    pub asks: [DepthLevel; 5],
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tick {
    pub quote_fields: Option<QuoteFields>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuoteFields {
    pub average_price_paise: i32,
    pub total_buy_quantity: u32,
    pub total_sell_quantity: u32,
    pub open_paise: i32,
    pub high_paise: i32,
    pub low_paise: i32,
    pub close_paise: i32,
}

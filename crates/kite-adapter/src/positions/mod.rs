use serde::Deserialize;
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub(crate) struct Position {
    pub exchange: String,
    pub tradingsymbol: String,
    pub instrument_token: u32,
    pub product: String,
    pub quantity: i64,
    pub overnight_quantity: i64,
    pub day_buy_quantity: i64,
    pub day_sell_quantity: i64,
}
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Positions {
    pub net: Vec<Position>,
    pub day: Vec<Position>,
}

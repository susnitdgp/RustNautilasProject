use serde::Deserialize;
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub(crate) struct Trade {
    pub trade_id: String,
    pub order_id: String,
    pub exchange: String,
    pub tradingsymbol: String,
    pub instrument_token: u32,
    pub product: String,
    pub transaction_type: String,
    pub quantity: i64,
}
